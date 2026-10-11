use sqlx::SqlitePool;

pub struct FeedItem {
    pub id: String,
    pub source_title: String,
    pub item_title: String,
}

fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        return s;
    }
    let mut end = max_len;
    while !s.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    &s[..end]
}

pub async fn insert_feed_item(
    db: &SqlitePool,
    id: &str,
    source_title: &str,
    item_title: &str,
    item_description: &str,
) -> Result<(), sqlx::Error> {
    let source_title = truncate_str(source_title, 256);
    let item_title = truncate_str(item_title, 256);
    let item_description = truncate_str(item_description, 2048);

    sqlx::query(
        "INSERT INTO feed_items(id, source_title, item_title, item_description) VALUES(?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET source_title=excluded.source_title, item_title=excluded.item_title, item_description=excluded.item_description"
    )
    .bind(id)
    .bind(source_title)
    .bind(item_title)
    .bind(item_description)
    .execute(db)
    .await?;

    Ok(())
}

#[must_use]
pub fn filter_feed_items(
    rows: Vec<(String, String, String, String)>,
    set_number: &str,
    set_name: &str,
    set_theme: &str,
) -> Vec<FeedItem> {
    let number_regex = regex::Regex::new(&format!(r"\b{}\b", regex::escape(set_number))).unwrap();

    let name_tokens: Vec<String> = set_name
        .to_lowercase()
        .split_whitespace()
        .map(std::string::ToString::to_string)
        .collect();
    let theme_tokens: Vec<String> = set_theme
        .to_lowercase()
        .split_whitespace()
        .map(std::string::ToString::to_string)
        .collect();

    let mut filtered_items = Vec::new();

    for (id, source_title, item_title, item_description) in rows {
        let title_lower = item_title.to_lowercase();
        let desc_lower = item_description.to_lowercase();

        if !number_regex.is_match(&item_title) && !number_regex.is_match(&item_description) {
            continue;
        }

        let is_match = if number_regex.is_match(&item_title) {
            true
        } else {
            name_tokens.iter().chain(theme_tokens.iter()).any(|token| {
                token.len() > 3 && (title_lower.contains(token) || desc_lower.contains(token))
            })
        };

        if is_match {
            filtered_items.push(FeedItem {
                id,
                source_title,
                item_title,
            });
            if filtered_items.len() >= 5 {
                break;
            }
        }
    }

    filtered_items
}

pub async fn search_feed_items(
    db: &SqlitePool,
    set_number: &str,
    set_name: &str,
    set_theme: &str,
) -> Result<Vec<FeedItem>, sqlx::Error> {
    let search_pattern = format!("%{set_number}%");
    let rows: Vec<(String, String, String, String)> =
        sqlx::query_as("SELECT id, source_title, item_title, item_description FROM feed_items WHERE item_title LIKE ? OR item_description LIKE ? LIMIT 20")
            .bind(&search_pattern)
            .bind(&search_pattern)
            .fetch_all(db)
            .await?;

    Ok(filter_feed_items(rows, set_number, set_name, set_theme))
}

pub async fn get_last_polled_at(db: &SqlitePool, url: &str) -> Option<chrono::NaiveDateTime> {
    let row: Option<(chrono::NaiveDateTime,)> =
        sqlx::query_as("SELECT last_polled_at FROM feed_polls WHERE url = ?")
            .bind(url)
            .fetch_optional(db)
            .await
            .unwrap_or(None);
    row.map(|(dt,)| dt)
}

/// Records that `url` was polled at `at` — the start of the polling pass, so
/// that everything polled in one pass falls due together again.
pub async fn mark_polled(
    db: &SqlitePool,
    url: &str,
    at: chrono::NaiveDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO feed_polls (url, last_polled_at) VALUES (?, ?) ON CONFLICT(url) DO UPDATE SET last_polled_at = excluded.last_polled_at")
        .bind(url)
        .bind(at)
        .execute(db)
        .await?;
    Ok(())
}

/// How long an article no feed lists any more is still remembered as seen.
const SIGHTING_RETENTION_DAYS: u32 = 30;

/// Whether a published feed still has to be polled for this run's baseline,
/// even though its interval has not elapsed.
///
/// True until the first attempt of the run: a failed attempt still moves
/// `last_polled_at` past `started_at`, so a dead feed is not retried every
/// pass — its baseline is then taken by the next regular poll.
pub async fn needs_baseline_poll(
    db: &SqlitePool,
    url: &str,
    run_id: i64,
    started_at: chrono::NaiveDateTime,
) -> bool {
    let row: Result<Option<(chrono::NaiveDateTime, Option<i64>)>, _> =
        sqlx::query_as("SELECT last_polled_at, baseline_run FROM feed_polls WHERE url = ?")
            .bind(url)
            .fetch_optional(db)
            .await;
    match row {
        Ok(None) => true,
        Ok(Some((last_polled_at, baseline_run))) => {
            baseline_run != Some(run_id) && last_polled_at < started_at
        }
        // A database that cannot answer is no reason to fetch every pass.
        Err(_) => false,
    }
}

/// Records the articles a poll of `feed_url` listed and returns the ids to
/// announce: those never seen on this feed before, in the order given.
///
/// The first call for a feed in a run (`run_id`) is its baseline and returns
/// nothing, so whatever was published before or while the bot was down is
/// never announced. All of it commits together — a failure leaves the feed
/// without a baseline rather than with half of one.
pub async fn record_feed_sightings(
    db: &SqlitePool,
    feed_url: &str,
    run_id: i64,
    item_ids: &[&str],
) -> Result<Vec<String>, sqlx::Error> {
    let mut tx = db.begin().await?;

    // Writes first, so the transaction takes the write lock up front. Reading
    // first and upgrading later can fail with SQLITE_BUSY straight away,
    // without the busy timeout applying.
    let created = sqlx::query("INSERT OR IGNORE INTO feed_polls (url, baseline_run) VALUES (?, ?)")
        .bind(feed_url)
        .bind(run_id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        == 1;
    let restamped = sqlx::query(
        "UPDATE feed_polls SET baseline_run = ? WHERE url = ? AND baseline_run IS NOT ?",
    )
    .bind(run_id)
    .bind(feed_url)
    .bind(run_id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    let is_baseline = created || restamped;

    let mut unseen = Vec::new();
    for id in item_ids {
        let inserted =
            sqlx::query("INSERT OR IGNORE INTO feed_sightings(feed_url, item_id) VALUES(?, ?)")
                .bind(feed_url)
                .bind(id)
                .execute(&mut *tx)
                .await?
                .rows_affected()
                == 1;
        if inserted {
            unseen.push((*id).to_string());
        } else {
            sqlx::query(
                "UPDATE feed_sightings SET last_seen_at = CURRENT_TIMESTAMP WHERE feed_url = ? AND item_id = ?",
            )
            .bind(feed_url)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
    }

    // Prunes this feed's own stale rows — everything it still lists was just
    // refreshed — and those of feeds without a baseline in this run, which
    // will take a silent one before announcing anything. A feed that does
    // have one but has been failing keeps its rows: pruning them would make
    // its whole listing look new the day it recovers.
    sqlx::query(
        "DELETE FROM feed_sightings
         WHERE last_seen_at < datetime('now', ?)
           AND (feed_url = ? OR feed_url NOT IN
                (SELECT url FROM feed_polls WHERE baseline_run = ?))",
    )
    .bind(format!("-{SIGHTING_RETENTION_DAYS} days"))
    .bind(feed_url)
    .bind(run_id)
    .execute(&mut *tx)
    .await?;

    if is_baseline {
        unseen.clear();
    }

    tx.commit().await?;
    Ok(unseen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_feed_items() {
        let rows = vec![
            (
                "1".to_string(),
                "Source".to_string(),
                "LEGO 77093 Review".to_string(),
                "A review of the new set.".to_string(),
            ),
            (
                "2".to_string(),
                "Source".to_string(),
                "Random Article".to_string(),
                "The number 177093 is not the set.".to_string(),
            ),
            (
                "3".to_string(),
                "Source".to_string(),
                "Other Article".to_string(),
                "This mentions 77093 but has nothing to do with it.".to_string(),
            ),
            (
                "4".to_string(),
                "Source".to_string(),
                "Zelda News".to_string(),
                "The great deku tree 77093 is amazing.".to_string(),
            ),
        ];

        let filtered = filter_feed_items(rows, "77093", "Great Deku Tree", "The Legend of Zelda");

        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].id, "1"); // Matches in title
        assert_eq!(filtered[1].id, "4"); // Matches in description AND has "great" or "deku"
    }

    async fn setup_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    async fn sightings(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM feed_sightings")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn the_first_poll_of_a_run_is_a_silent_baseline() {
        let pool = setup_db().await;

        let fresh = record_feed_sightings(&pool, "feed", 1, &["a", "b"])
            .await
            .unwrap();

        assert!(fresh.is_empty());
        assert_eq!(sightings(&pool).await, 2);
    }

    #[tokio::test]
    async fn later_polls_of_the_run_report_only_unseen_articles() {
        let pool = setup_db().await;
        record_feed_sightings(&pool, "feed", 1, &["a"])
            .await
            .unwrap();

        let fresh = record_feed_sightings(&pool, "feed", 1, &["c", "b", "a", "c"])
            .await
            .unwrap();
        assert_eq!(fresh, ["c", "b"]);

        // Reported once, and never again — even after dropping off the feed.
        let again = record_feed_sightings(&pool, "feed", 1, &["c"])
            .await
            .unwrap();
        assert!(again.is_empty());
        let back = record_feed_sightings(&pool, "feed", 1, &["a"])
            .await
            .unwrap();
        assert!(back.is_empty());
    }

    #[tokio::test]
    async fn an_empty_baseline_still_counts_as_one() {
        let pool = setup_db().await;
        record_feed_sightings(&pool, "feed", 1, &[]).await.unwrap();

        let fresh = record_feed_sightings(&pool, "feed", 1, &["a"])
            .await
            .unwrap();

        assert_eq!(fresh, ["a"]);
    }

    #[tokio::test]
    async fn articles_published_while_the_bot_was_down_are_not_announced() {
        let pool = setup_db().await;
        record_feed_sightings(&pool, "feed", 1, &["a"])
            .await
            .unwrap();

        // Restart: "b" appeared in between, so it belongs to the new baseline.
        let baseline = record_feed_sightings(&pool, "feed", 2, &["b", "a"])
            .await
            .unwrap();
        assert!(baseline.is_empty());

        let fresh = record_feed_sightings(&pool, "feed", 2, &["c", "b", "a"])
            .await
            .unwrap();
        assert_eq!(fresh, ["c"]);
    }

    #[tokio::test]
    async fn each_feed_has_its_own_baseline_and_sightings() {
        let pool = setup_db().await;
        record_feed_sightings(&pool, "one", 1, &["a"])
            .await
            .unwrap();
        record_feed_sightings(&pool, "one", 1, &["b"])
            .await
            .unwrap();

        // "b" is known on feed one, but this is feed two's first poll.
        let baseline = record_feed_sightings(&pool, "two", 1, &["b"])
            .await
            .unwrap();
        assert!(baseline.is_empty());
        let fresh = record_feed_sightings(&pool, "two", 1, &["a"])
            .await
            .unwrap();
        assert_eq!(fresh, ["a"]);
    }

    async fn age_sightings(pool: &SqlitePool) {
        sqlx::query("UPDATE feed_sightings SET last_seen_at = datetime('now', '-31 days')")
            .execute(pool)
            .await
            .unwrap();
    }

    async fn sighted(pool: &SqlitePool) -> Vec<String> {
        sqlx::query_scalar("SELECT item_id FROM feed_sightings ORDER BY item_id")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn sightings_a_feed_no_longer_lists_are_pruned() {
        let pool = setup_db().await;
        record_feed_sightings(&pool, "feed", 1, &["old", "kept"])
            .await
            .unwrap();
        age_sightings(&pool).await;

        // "kept" is still listed, which refreshes it; "old" has aged out.
        record_feed_sightings(&pool, "feed", 1, &["kept"])
            .await
            .unwrap();

        assert_eq!(sighted(&pool).await, ["kept"]);
    }

    #[tokio::test]
    async fn a_failing_feed_keeps_its_sightings_for_the_run() {
        let pool = setup_db().await;
        record_feed_sightings(&pool, "failing", 1, &["a"])
            .await
            .unwrap();
        age_sightings(&pool).await;

        // Another feed's poll must not prune it: once "failing" recovers in
        // this run, "a" would be announced as new.
        record_feed_sightings(&pool, "other", 1, &[]).await.unwrap();

        let recovered = record_feed_sightings(&pool, "failing", 1, &["a"])
            .await
            .unwrap();
        assert!(recovered.is_empty());
    }

    #[tokio::test]
    async fn sightings_of_feeds_from_earlier_runs_are_pruned() {
        let pool = setup_db().await;
        record_feed_sightings(&pool, "removed", 1, &["stale"])
            .await
            .unwrap();
        age_sightings(&pool).await;

        // A later run no longer polls "removed" at all.
        record_feed_sightings(&pool, "feed", 2, &["a"])
            .await
            .unwrap();

        assert_eq!(sighted(&pool).await, ["a"]);
    }

    #[tokio::test]
    async fn marking_a_poll_keeps_the_baseline() {
        let pool = setup_db().await;
        let now = chrono::Utc::now().naive_utc();
        record_feed_sightings(&pool, "feed", 1, &["a"])
            .await
            .unwrap();
        mark_polled(&pool, "feed", now).await.unwrap();

        let fresh = record_feed_sightings(&pool, "feed", 1, &["b"])
            .await
            .unwrap();

        assert_eq!(fresh, ["b"]);
        assert_eq!(get_last_polled_at(&pool, "feed").await, Some(now));
    }

    #[tokio::test]
    async fn a_baseline_poll_is_needed_once_per_run() {
        let pool = setup_db().await;
        let now = chrono::Utc::now().naive_utc();
        let later = now + chrono::Duration::hours(1);

        // Never polled at all.
        assert!(needs_baseline_poll(&pool, "feed", 1, now).await);

        // Polled by an earlier run: the new run, started later, needs its own.
        record_feed_sightings(&pool, "feed", 1, &["a"])
            .await
            .unwrap();
        mark_polled(&pool, "feed", now).await.unwrap();
        assert!(!needs_baseline_poll(&pool, "feed", 1, later).await);
        assert!(needs_baseline_poll(&pool, "feed", 2, later).await);
    }

    #[tokio::test]
    async fn a_failed_baseline_attempt_is_not_retried_every_pass() {
        let pool = setup_db().await;
        let started_at = chrono::Utc::now().naive_utc() - chrono::Duration::hours(1);

        // The attempt failed, so only the poll time moved — after startup.
        mark_polled(&pool, "feed", chrono::Utc::now().naive_utc())
            .await
            .unwrap();

        assert!(!needs_baseline_poll(&pool, "feed", 2, started_at).await);
    }

    #[tokio::test]
    async fn an_unreadable_database_does_not_force_a_poll() {
        let pool = setup_db().await;
        pool.close().await;

        assert!(!needs_baseline_poll(&pool, "feed", 1, chrono::Utc::now().naive_utc()).await);
    }
}
