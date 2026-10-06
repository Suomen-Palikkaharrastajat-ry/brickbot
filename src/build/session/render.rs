//! The debounced render loop and posting of renders and exports.

use super::{
    Arc, BuildCommandConfig, ChannelId, Context, CreateAttachment, CreateMessage, Duration,
    History, LdrawLibrary, RenderOptions, RenderPlan, SceneMode, SceneTriangle, build_cmd, db,
    debounce_delay, library_for, plan_render, render_slots, scene,
};

/// Per-session render pacing: when the last picture went out, and the task
/// waiting to send the next one.
pub(super) struct RenderState {
    pub(super) last_render: std::time::Instant,
    pub(super) pending: Option<tokio::task::JoinHandle<()>>,
}

static RENDER_STATE: std::sync::OnceLock<
    moka::future::Cache<String, Arc<tokio::sync::Mutex<RenderState>>>,
> = std::sync::OnceLock::new();

pub(super) async fn render_state(session_id: &str) -> Arc<tokio::sync::Mutex<RenderState>> {
    RENDER_STATE
        .get_or_init(|| {
            moka::future::Cache::builder()
                .max_capacity(1_000)
                .time_to_idle(Duration::from_hours(1))
                .build()
        })
        .get_with(session_id.to_string(), async {
            Arc::new(tokio::sync::Mutex::new(RenderState {
                // Long enough ago that the first command in a session renders
                // straight away rather than waiting out a window.
                last_render: std::time::Instant::now()
                    .checked_sub(Duration::from_hours(1))
                    .unwrap_or_else(std::time::Instant::now),
                pending: None,
            }))
        })
        .await
}

/// Drops any render that has not gone out yet.
pub(super) async fn cancel_pending_render(session_id: &str) {
    let state = render_state(session_id).await;
    let mut state = state.lock().await;
    if let Some(handle) = state.pending.take() {
        handle.abort();
    }
}

/// Renders now, or arranges for one render to cover this command and whatever
/// else arrives in the next `render_debounce_ms`.
///
/// Leading edge plus trailing edge: a lone command draws immediately, so the
/// thread feels responsive, and a burst adds exactly one more picture at the
/// end of the window rather than one per message. The trailing render reloads
/// the history when it fires, so it shows everything that landed meanwhile.
pub(super) async fn schedule_render(
    ctx: &Context,
    pool: &sqlx::SqlitePool,
    session: &db::build::BuildSession,
    cfg: &BuildCommandConfig,
    locale: &str,
    channel_id: ChannelId,
) {
    let state = render_state(&session.id).await;
    let mut state = state.lock().await;

    if let Some(handle) = state.pending.take() {
        // A later command supersedes the render already waiting.
        handle.abort();
    }

    let wait = match plan_render(state.last_render.elapsed(), debounce_delay(cfg)) {
        RenderPlan::Now => {
            state.last_render = std::time::Instant::now();
            None
        }
        RenderPlan::After(wait) => Some(wait),
    };

    let Some(wait) = wait else {
        drop(state);
        let history = match db::build::load_history(pool, &session.id, &session.model_name).await {
            Ok(history) => history,
            Err(e) => {
                tracing::error!("build: could not load history to render: {e}");
                return;
            }
        };
        let library = library_for(ctx).await;
        post_render(
            ctx,
            channel_id,
            &history,
            &library,
            cfg,
            &session.id,
            locale,
        )
        .await;
        return;
    };

    let ctx = ctx.clone();
    let pool = pool.clone();
    let session = session.clone();
    let cfg = cfg.clone();
    let locale = locale.to_string();
    let state_handle = Arc::clone(&render_state(&session.id).await);

    state.pending = Some(tokio::spawn(async move {
        tokio::time::sleep(wait).await;
        let Ok(history) = db::build::load_history(&pool, &session.id, &session.model_name).await
        else {
            return;
        };
        let library = library_for(&ctx).await;
        post_render(
            &ctx,
            channel_id,
            &history,
            &library,
            &cfg,
            &session.id,
            &locale,
        )
        .await;
        let mut state = state_handle.lock().await;
        state.last_render = std::time::Instant::now();
        state.pending = None;
    }));
}

/// Builds the scene and renders it, off the async runtime's worker threads.
pub(super) async fn render_model(
    history: &History,
    library: &LdrawLibrary,
    cfg: &BuildCommandConfig,
) -> Option<Vec<u8>> {
    let model = history.materialize();
    if model.is_empty() {
        return None;
    }

    let mode = if library.is_available() {
        SceneMode::Meshes
    } else {
        SceneMode::Boxes
    };
    let (triangles, report) = scene::build_scene(&model, library, mode).await;
    if report.is_approximate() {
        tracing::debug!("build: {} placement(s) drawn as boxes", report.boxed);
    }

    let options = RenderOptions {
        width: cfg.render_width,
        height: cfg.render_height,
        supersample: cfg.supersample,
        ..RenderOptions::default()
    };

    // Rendering is CPU-bound; keep it off the gateway's threads and cap how
    // many can run at once.
    let _permit = render_slots().acquire().await.ok()?;
    let triangles: Vec<SceneTriangle> = triangles;
    tokio::task::spawn_blocking(move || crate::render::render_png(&triangles, &options))
        .await
        .ok()?
        .ok()
}

pub(super) async fn post_render(
    ctx: &Context,
    channel_id: ChannelId,
    history: &History,
    library: &LdrawLibrary,
    cfg: &BuildCommandConfig,
    session_id: &str,
    locale: &str,
) {
    let _ = channel_id.broadcast_typing(&ctx.http).await;
    let Some(png) = render_model(history, library, cfg).await else {
        return;
    };

    let model = history.materialize();
    let message = CreateMessage::new()
        .content(build_cmd::render_caption(
            &model,
            history.revision(),
            locale,
        ))
        .add_file(CreateAttachment::bytes(png, "model.png"))
        .components(vec![build_cmd::build_session_buttons(
            session_id,
            locale,
            cfg.enable_io_export,
        )]);

    if let Err(e) = channel_id.send_message(&ctx.http, message).await {
        tracing::error!("build: failed to post render: {e}");
    }
}

pub(super) async fn post_export(
    ctx: &Context,
    channel_id: ChannelId,
    history: &History,
    library: &LdrawLibrary,
    cfg: &BuildCommandConfig,
    locale: &str,
) {
    let model = history.materialize();
    if model.is_empty() {
        let _ = channel_id
            .say(
                &ctx.http,
                rust_i18n::t!("command.build.export_empty", locale = locale).to_string(),
            )
            .await;
        return;
    }

    let thumbnail = render_model(history, library, cfg)
        .await
        .unwrap_or_default();
    let bytes = crate::build::export::to_io(&model, &thumbnail);
    let name = format!("{}.io", crate::build::export::file_stem(&model.name));

    let message = CreateMessage::new()
        .content(
            rust_i18n::t!(
                "command.build.export_ready",
                locale = locale,
                name = &model.name
            )
            .to_string(),
        )
        .add_file(CreateAttachment::bytes(bytes, name));

    if let Err(e) = channel_id.send_message(&ctx.http, message).await {
        tracing::error!("build: failed to post export: {e}");
    }
}
