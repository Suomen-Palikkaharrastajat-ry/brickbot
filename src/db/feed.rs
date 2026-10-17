#![allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
use sqlx::SqlitePool;

type PendingRow = (
    i64,
    i64,
    String,
    String,
    String,
    String,
    Option<chrono::DateTime<chrono::Utc>>,
);

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

/// An unseen article queued for a channel waiting to be announced at its scheduled time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAnnouncement {
    pub channel_id: u64,
    pub guild_id: u64,
    pub item_id: String,
    pub feed_url: String,
    pub source_title: String,
    pub item_title: String,
    pub published_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Queues unseen articles for a channel. Duplicate items are silently ignored.
pub async fn queue_pending_announcements(
    db: &SqlitePool,
    announcements: &[PendingAnnouncement],
) -> Result<(), sqlx::Error> {
    if announcements.is_empty() {
        return Ok(());
    }
    let mut tx = db.begin().await?;
    for a in announcements {
        let channel_id = a.channel_id as i64;
        let guild_id = a.guild_id as i64;
        sqlx::query(
            "INSERT INTO feed_pending_announcements
             (channel_id, guild_id, item_id, feed_url, source_title, item_title, published_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(channel_id, item_id) DO NOTHING",
        )
        .bind(channel_id)
        .bind(guild_id)
        .bind(&a.item_id)
        .bind(&a.feed_url)
        .bind(&a.source_title)
        .bind(&a.item_title)
        .bind(a.published_at)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Fetches all pending announcements queued for a channel in arrival order.
/// `rowid`, not `created_at`: the timestamp has whole seconds only, so it
/// cannot order the articles one pass queues together.
pub async fn get_pending_announcements(
    db: &SqlitePool,
    channel_id: u64,
) -> Result<Vec<PendingAnnouncement>, sqlx::Error> {
    let rows: Vec<PendingRow> = sqlx::query_as(
        "SELECT channel_id, guild_id, item_id, feed_url, source_title, item_title, published_at
         FROM feed_pending_announcements
         WHERE channel_id = ?
         ORDER BY rowid ASC",
    )
    .bind(channel_id as i64)
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(cid, gid, item_id, feed_url, source_title, item_title, published_at)| {
                PendingAnnouncement {
                    channel_id: cid as u64,
                    guild_id: gid as u64,
                    item_id,
                    feed_url,
                    source_title,
                    item_title,
                    published_at,
                }
            },
        )
        .collect())
}

/// Deletes announced items from a channel's pending queue.
pub async fn delete_pending_announcements(
    db: &SqlitePool,
    channel_id: u64,
    item_ids: &[&str],
) -> Result<u64, sqlx::Error> {
    if item_ids.is_empty() {
        return Ok(0);
    }
    let mut tx = db.begin().await?;
    let mut deleted = 0;
    for id in item_ids {
        deleted += sqlx::query(
            "DELETE FROM feed_pending_announcements WHERE channel_id = ? AND item_id = ?",
        )
        .bind(channel_id as i64)
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    }
    tx.commit().await?;
    Ok(deleted)
}

/// Every channel with something queued, whether or not it is still configured.
pub async fn pending_announcement_channels(db: &SqlitePool) -> Result<Vec<u64>, sqlx::Error> {
    let rows: Vec<(i64,)> =
        sqlx::query_as("SELECT DISTINCT channel_id FROM feed_pending_announcements")
            .fetch_all(db)
            .await?;
    Ok(rows.into_iter().map(|(id,)| id as u64).collect())
}

/// Drops everything queued for a channel no longer configured.
pub async fn clear_pending_announcements(
    db: &SqlitePool,
    channel_id: u64,
) -> Result<u64, sqlx::Error> {
    Ok(
        sqlx::query("DELETE FROM feed_pending_announcements WHERE channel_id = ?")
            .bind(channel_id as i64)
            .execute(db)
            .await?
            .rows_affected(),
    )
}

/// When a channel last completed a scheduled announcement run.
pub async fn get_channel_last_announced_at(
    db: &SqlitePool,
    channel_id: u64,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, sqlx::Error> {
    let row: Option<(chrono::DateTime<chrono::Utc>,)> = sqlx::query_as(
        "SELECT last_announced_at FROM feed_channel_announcements WHERE channel_id = ?",
    )
    .bind(channel_id as i64)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(dt,)| dt))
}

/// Marks that a scheduled announcement run completed for `channel_id` at `at`.
pub async fn mark_channel_announced(
    db: &SqlitePool,
    channel_id: u64,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO feed_channel_announcements (channel_id, last_announced_at)
         VALUES (?, ?)
         ON CONFLICT(channel_id) DO UPDATE SET last_announced_at = excluded.last_announced_at",
    )
    .bind(channel_id as i64)
    .bind(at)
    .execute(db)
    .await?;
    Ok(())
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

    #[tokio::test]
    async fn pending_announcements_can_be_queued_retrieved_and_deleted() {
        let pool = setup_db().await;
        let published = chrono::DateTime::parse_from_rfc3339("2026-10-17T12:00:00Z")
            .unwrap()
            .to_utc();

        let item1 = PendingAnnouncement {
            channel_id: 100,
            guild_id: 10,
            item_id: "https://example.com/item1".to_string(),
            feed_url: "https://example.com/feed".to_string(),
            source_title: "Example Source".to_string(),
            item_title: "Item 1".to_string(),
            published_at: Some(published),
        };
        let item2 = PendingAnnouncement {
            channel_id: 100,
            guild_id: 10,
            item_id: "https://example.com/item2".to_string(),
            feed_url: "https://example.com/feed".to_string(),
            source_title: "Example Source".to_string(),
            item_title: "Item 2".to_string(),
            published_at: None,
        };

        queue_pending_announcements(&pool, &[item1.clone(), item2.clone()])
            .await
            .unwrap();

        // Duplicate queueing is ignored (deduplicated by channel_id and item_id)
        queue_pending_announcements(&pool, std::slice::from_ref(&item1))
            .await
            .unwrap();

        let pending = get_pending_announcements(&pool, 100).await.unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].item_id, item1.item_id);
        assert_eq!(pending[0].source_title, item1.source_title);
        assert_eq!(pending[0].published_at, Some(published));
        assert_eq!(pending[1].item_id, item2.item_id);
        assert_eq!(pending[1].published_at, None);

        // Another channel has no pending announcements
        let other_pending = get_pending_announcements(&pool, 200).await.unwrap();
        assert!(other_pending.is_empty());

        // Deleting item1 removes only item1
        delete_pending_announcements(&pool, 100, &[&item1.item_id])
            .await
            .unwrap();
        let remaining = get_pending_announcements(&pool, 100).await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].item_id, item2.item_id);
    }

    #[tokio::test]
    async fn pending_announcements_keep_arrival_order_within_a_second() {
        let pool = setup_db().await;
        let item = |id: &str, channel_id| PendingAnnouncement {
            channel_id,
            guild_id: 1,
            item_id: id.to_string(),
            feed_url: "https://example.com/feed".to_string(),
            source_title: "Source".to_string(),
            item_title: id.to_string(),
            published_at: None,
        };
        queue_pending_announcements(&pool, &[item("z", 1), item("a", 1), item("m", 2)])
            .await
            .unwrap();

        let ids: Vec<String> = get_pending_announcements(&pool, 1)
            .await
            .unwrap()
            .into_iter()
            .map(|a| a.item_id)
            .collect();
        assert_eq!(ids, ["z", "a"]);

        let mut channels = pending_announcement_channels(&pool).await.unwrap();
        channels.sort_unstable();
        assert_eq!(channels, [1, 2]);

        assert_eq!(clear_pending_announcements(&pool, 1).await.unwrap(), 2);
        assert_eq!(pending_announcement_channels(&pool).await.unwrap(), [2]);
    }

    #[tokio::test]
    async fn channel_announcements_last_announced_tracking() {
        let pool = setup_db().await;
        let channel_id = 42;

        assert_eq!(
            get_channel_last_announced_at(&pool, channel_id)
                .await
                .unwrap(),
            None
        );

        let t1 = chrono::DateTime::parse_from_rfc3339("2026-10-17T08:00:00Z")
            .unwrap()
            .to_utc();
        mark_channel_announced(&pool, channel_id, t1).await.unwrap();

        assert_eq!(
            get_channel_last_announced_at(&pool, channel_id)
                .await
                .unwrap(),
            Some(t1)
        );

        let t2 = chrono::DateTime::parse_from_rfc3339("2026-10-18T08:00:00Z")
            .unwrap()
            .to_utc();
        mark_channel_announced(&pool, channel_id, t2).await.unwrap();

        assert_eq!(
            get_channel_last_announced_at(&pool, channel_id)
                .await
                .unwrap(),
            Some(t2)
        );
    }
}
