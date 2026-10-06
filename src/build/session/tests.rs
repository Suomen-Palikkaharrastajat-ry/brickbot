use super::proposals::{PROPOSAL_HIGHLIGHT, PROPOSAL_TINT, candidate_soup};
use super::render::render_state;
use super::*;

fn empty_library() -> LdrawLibrary {
    LdrawLibrary::new(
        std::env::temp_dir().join("brickbot-session-no-library"),
        crate::ldraw::resolve::ResolveLimits::default(),
    )
}

#[tokio::test]
async fn a_candidate_tile_keeps_the_parts_real_colour_not_a_flat_highlight() {
    use crate::build::model::{PartKind, PartRef};
    use crate::build::ops::Placed;
    use crate::build::propose::{Candidate, Origin};
    use crate::build::units::{IVec3, Orientation};

    let white = 15;
    let candidate = Candidate {
        placements: vec![Placed {
            part: PartRef::new("3005", "Brick 1 x 1", (1, 1), PartKind::Brick),
            color: white,
            pos: IVec3::ZERO,
            orientation: Orientation::DEFAULT,
        }],
        origin: Origin::Ground,
        score: 100,
    };

    // No library, so this is box mode — deterministic, no filesystem
    // dependency, and exactly what `build_scene` gives the same
    // placement once it is actually part of the model.
    let library = empty_library();
    let soup = candidate_soup(&[], &library, false, &candidate).await;
    assert!(!soup.is_empty());

    let expected = scene::blend(
        crate::ldraw::colors::resolve(white),
        PROPOSAL_HIGHLIGHT,
        PROPOSAL_TINT,
    );
    assert!(
        soup.iter().all(|t| t.color == expected),
        "a white brick's tile must still read as white, tinted — not the flat highlight colour"
    );
    // And never the flat highlight on its own: white nudged toward
    // magenta is not magenta.
    assert!(soup.iter().all(|t| t.color != PROPOSAL_HIGHLIGHT));
}

#[test]
fn a_render_waits_until_the_debounce_window_has_passed() {
    // Four people typing at once should produce one picture, not four.
    assert!(!should_render_now(0, 1200));
    assert!(!should_render_now(1199, 1200));
    assert!(should_render_now(1200, 1200));
    assert!(should_render_now(5000, 1200));
}

#[test]
fn a_zero_debounce_always_renders() {
    assert!(should_render_now(0, 0));
}

#[test]
fn the_debounce_delay_comes_from_config() {
    let cfg = BuildCommandConfig {
        render_debounce_ms: 900,
        ..BuildCommandConfig::default()
    };
    assert_eq!(debounce_delay(&cfg), Duration::from_millis(900));
}

#[test]
fn a_lone_command_draws_straight_away() {
    // Leading edge: the thread must not feel sluggish when one person is
    // building on their own.
    assert_eq!(
        plan_render(Duration::from_secs(30), Duration::from_millis(1200)),
        RenderPlan::Now
    );
}

#[test]
fn a_burst_is_coalesced_into_one_later_render() {
    // Trailing edge: each command inside the window replaces the render
    // waiting to go out, so four people typing at once get one picture.
    assert_eq!(
        plan_render(Duration::from_millis(0), Duration::from_millis(1200)),
        RenderPlan::After(Duration::from_millis(1200))
    );
    assert_eq!(
        plan_render(Duration::from_millis(900), Duration::from_millis(1200)),
        RenderPlan::After(Duration::from_millis(300)),
        "the wait shrinks as the window runs down"
    );
}

#[test]
fn a_zero_debounce_never_waits() {
    assert_eq!(plan_render(Duration::ZERO, Duration::ZERO), RenderPlan::Now);
}

#[tokio::test]
async fn a_fresh_session_renders_immediately() {
    let state = render_state("debounce-fresh").await;
    let since = state.lock().await.last_render.elapsed();
    assert_eq!(
        plan_render(since, Duration::from_millis(1200)),
        RenderPlan::Now,
        "the first command of a session must not wait out a window"
    );
}

#[tokio::test]
async fn cancelling_clears_the_waiting_render() {
    let state = render_state("debounce-cancel").await;
    {
        let mut state = state.lock().await;
        state.pending = Some(tokio::spawn(async {
            tokio::time::sleep(Duration::from_mins(1)).await;
        }));
    }
    cancel_pending_render("debounce-cancel").await;
    assert!(state.lock().await.pending.is_none());
}

#[test]
fn there_are_a_bounded_number_of_render_slots() {
    // The cap is what keeps a busy guild from starving the gateway.
    assert!(render_slots().available_permits() <= 2);
}

// --- Persistence -------------------------------------------------------

async fn setup_db() -> sqlx::SqlitePool {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    pool
}

/// Loads, interprets and persists one message, the way
/// [`handle_thread_message`] does but without a gateway.
async fn build(pool: &sqlx::SqlitePool, session_id: &str, author: &str, text: &str) {
    let history = db::build::load_history(pool, session_id, "Castle")
        .await
        .unwrap();
    let builders = dispatch::Builders::new([
        ("alice".to_string(), "111".to_string()),
        ("bob".to_string(), "222".to_string()),
    ]);
    let outcome = dispatch::process_message(
        &history,
        text,
        "en-US",
        &builders,
        author,
        MaxModelSize::cube(10),
    );
    persist(pool, session_id, &history, &outcome, author)
        .await
        .unwrap();
}

async fn model_of(pool: &sqlx::SqlitePool, session_id: &str) -> crate::build::Model {
    db::build::load_history(pool, session_id, "Castle")
        .await
        .unwrap()
        .materialize()
}

#[tokio::test]
async fn an_undo_then_a_command_keeps_the_new_brick() {
    // Regression: `persist` derived the write position from the loaded
    // cursor, so the new operation landed on top of the one the undo had
    // just freed and the cursor was then set below it — the yellow brick
    // disappeared.
    let pool = setup_db().await;
    let s = db::build::create_build_session(&pool, "g", "t", "alice", "Castle", 24, None)
        .await
        .unwrap();

    build(&pool, &s.id, "alice", "Add a red 2x4 brick at origin.").await;
    build(&pool, &s.id, "bob", "Add a blue 2x4 brick.").await;
    build(&pool, &s.id, "carol", "Undo. Add a yellow 2x4 brick.").await;

    let model = model_of(&pool, &s.id).await;
    assert_eq!(model.len(), 2);
    assert_eq!(model.placements[0].color, 4, "the red brick is untouched");
    assert_eq!(model.placements[1].color, 14, "the yellow brick survived");
}

#[tokio::test]
async fn undo_and_redo_round_trip_through_the_database() {
    let pool = setup_db().await;
    let s = db::build::create_build_session(&pool, "g", "t", "alice", "Castle", 24, None)
        .await
        .unwrap();

    build(&pool, &s.id, "alice", "Add a red 2x4 brick at origin.").await;
    build(&pool, &s.id, "alice", "Add a blue 2x4 brick.").await;
    assert_eq!(model_of(&pool, &s.id).await.len(), 2);

    build(&pool, &s.id, "alice", "Undo.").await;
    assert_eq!(model_of(&pool, &s.id).await.len(), 1);

    build(&pool, &s.id, "alice", "Redo.").await;
    assert_eq!(
        model_of(&pool, &s.id).await.len(),
        2,
        "redo brings the brick back"
    );
}

#[tokio::test]
async fn a_paused_sentence_is_parked_and_later_cleared() {
    let pool = setup_db().await;
    let s = db::build::create_build_session(&pool, "g", "t", "alice", "Castle", 24, None)
        .await
        .unwrap();

    // A part the catalogue cannot answer parks the sentence for the
    // dropdown to pick up.
    build(&pool, &s.id, "alice", "Add a red 3x12 brick at origin.").await;
    let parked = db::build::get_active_session_by_channel(&pool, "t")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        parked.pending_sentence.as_deref(),
        Some("Add a red 3x12 brick at origin")
    );
    // The shape it asked for travels with it, so the part chosen from
    // the dropdown keeps brick height rather than falling back to it by
    // coincidence.
    assert_eq!(
        parked.pending_kind,
        Some(super::super::model::PartKind::Brick)
    );
    assert!(
        model_of(&pool, &s.id).await.is_empty(),
        "nothing applied yet"
    );

    // Anything that does go through moves the model on, so the parked
    // sentence is no longer meaningful.
    build(&pool, &s.id, "bob", "Add a blue 2x4 brick at origin.").await;
    assert!(
        db::build::get_active_session_by_channel(&pool, "t")
            .await
            .unwrap()
            .unwrap()
            .pending_sentence
            .is_none()
    );
}

#[tokio::test]
async fn the_session_lock_serialises_writers() {
    // The lock is the only thing standing between two simultaneous
    // builders and a lost brick, so assert it actually excludes.
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let tasks: Vec<_> = (0..8)
        .map(|_| {
            let counter = Arc::clone(&counter);
            let peak = Arc::clone(&peak);
            tokio::spawn(async move {
                let lock = session_lock("session-a").await;
                let _guard = lock.lock().await;
                let inside = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                peak.fetch_max(inside, std::sync::atomic::Ordering::SeqCst);
                tokio::task::yield_now().await;
                counter.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            })
        })
        .collect();
    for task in tasks {
        task.await.unwrap();
    }

    assert_eq!(
        peak.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "only one writer at a time"
    );
}

#[tokio::test]
async fn different_sessions_do_not_block_each_other() {
    let a = session_lock("session-b").await;
    let b = session_lock("session-c").await;
    let _held = a.lock().await;
    // Would deadlock if the two sessions shared a lock.
    assert!(b.try_lock().is_ok());
    assert!(Arc::ptr_eq(&a, &session_lock("session-b").await));
}
