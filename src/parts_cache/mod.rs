//! A sticky, on-disk cache of Rebrickable part lookups.
//!
//! We cannot host a comprehensive LEGO catalogue, and Discord fires
//! autocomplete on every keystroke with a three-second deadline — so the
//! network can never be on that path. The answer is to keep what the API has
//! already told us. Every `/part` search the bot performs today is thrown
//! away; kept instead, they accumulate into a local index that answers
//! instantly and survives restarts.
//!
//! Two lookup paths, and the second is what makes it feel good:
//!
//! * [`PartCache::lookup`] replays a query someone has run before.
//! * [`PartCache::matches`] prefix- and substring-matches over *every part
//!   ever cached*, so a query nobody has typed before still finds something as
//!   long as the part itself has been seen. The index ends up better than the
//!   sum of the queries put into it.
//!
//! Lives in its own database (`data/cache.db`) because all of it is derived
//! and disposable — deleting the file is a clean rebuild, where `bot.db` holds
//! sessions and revisions that cannot be regenerated.

pub mod warm;

use crate::brick::{RebrickablePart, RebrickablePartSearchResult};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::str::FromStr;

/// A part as the cache holds it: exactly what an autocomplete choice needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedPart {
    pub part_num: String,
    pub name: String,
}

impl From<RebrickablePartSearchResult> for CachedPart {
    fn from(result: RebrickablePartSearchResult) -> Self {
        Self {
            part_num: result.part_num,
            name: result.name,
        }
    }
}

/// How long a cache entry stays good.
///
/// Tiered by how volatile the thing actually is. Part details are absent
/// because they never expire at all.
#[derive(Debug, Clone, Copy)]
pub struct Ttl {
    /// A search that found something. Rebrickable does add parts, so this is
    /// long rather than forever.
    pub search_days: i64,
    /// A search that found nothing. Short, so a typo is not pinned for a
    /// month — but not zero, or every keystroke would re-ask.
    pub miss_mins: i64,
}

impl Default for Ttl {
    fn default() -> Self {
        Self {
            search_days: 30,
            miss_mins: 60,
        }
    }
}

/// Folds a query so that `2X4 Brick` and `2x4  brick` are one cache entry.
///
/// Mirrors `crate::ldraw::colors::normalize`, which does the same job for
/// colour phrases.
#[must_use]
pub fn normalize(query: &str) -> String {
    query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

pub struct PartCache {
    pool: SqlitePool,
    /// Carried here rather than passed to every write, so no call site can
    /// record an entry with a different expiry than the rest.
    ttl: Ttl,
}

impl PartCache {
    /// Opens (and creates) the cache database.
    ///
    /// Runs looser pragmas than `bot.db` would tolerate: this file holds
    /// nothing that cannot be re-fetched, so trading durability for speed is
    /// the right way round. Note that `SQLite` creates the file but not its
    /// parent directory.
    pub async fn open(path: &str, ttl: Ttl) -> anyhow::Result<Self> {
        let options = SqliteConnectOptions::from_str(path)?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal);

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;

        Self::migrate(&pool).await?;
        Ok(Self { pool, ttl })
    }

    /// Closes the pool, checkpointing the database. Used before the process
    /// re-executes itself on a configuration reload.
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Wraps an existing pool, for tests.
    pub async fn from_pool(pool: SqlitePool, ttl: Ttl) -> anyhow::Result<Self> {
        Self::migrate(&pool).await?;
        Ok(Self { pool, ttl })
    }

    /// The cache's **own** migrations. Pointing this at `./migrations` would
    /// build the whole bot schema in the cache file.
    async fn migrate(pool: &SqlitePool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations_cache").run(pool).await?;
        Ok(())
    }

    /// A search someone has run before, if it has not expired.
    ///
    /// `Some(vec![])` means the search genuinely found nothing and that answer
    /// is still fresh — which is different from `None`, "we do not know".
    pub async fn lookup(&self, query: &str) -> anyhow::Result<Option<Vec<CachedPart>>> {
        let key = normalize(query);
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT part_nums_json FROM part_searches
             WHERE query = ? AND expires_at > CURRENT_TIMESTAMP",
        )
        .bind(&key)
        .fetch_optional(&self.pool)
        .await?;

        let Some((json,)) = row else {
            return Ok(None);
        };
        let part_nums: Vec<String> = serde_json::from_str(&json).unwrap_or_default();
        if part_nums.is_empty() {
            return Ok(Some(Vec::new()));
        }

        // Resolve the numbers back to names through the details table, so a
        // renamed part is reflected without re-running the search.
        let mut parts = Vec::with_capacity(part_nums.len());
        for part_num in part_nums {
            let found: Option<(String, String)> =
                sqlx::query_as("SELECT part_num, name FROM part_details WHERE part_num = ?")
                    .bind(&part_num)
                    .fetch_optional(&self.pool)
                    .await?;
            if let Some((part_num, name)) = found {
                parts.push(CachedPart { part_num, name });
            }
        }
        Ok(Some(parts))
    }

    /// Every cached part whose name or number matches, best first.
    ///
    /// This is the path that answers a query nobody has run before. A name
    /// that *starts* with the query ranks above one that merely contains it,
    /// and an exact part number beats both — typing `3001` should not bury the
    /// brick it names under everything mentioning it.
    pub async fn matches(&self, query: &str, limit: usize) -> anyhow::Result<Vec<CachedPart>> {
        let key = normalize(query);
        if key.is_empty() {
            return Ok(Vec::new());
        }
        let pattern = format!("%{key}%");
        let prefix = format!("{key}%");

        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT part_num, name FROM part_details
             WHERE name_folded LIKE ?1 OR LOWER(part_num) LIKE ?2
             ORDER BY
                 CASE
                     WHEN LOWER(part_num) = ?3 THEN 0
                     WHEN name_folded LIKE ?2 THEN 1
                     ELSE 2
                 END,
                 LENGTH(name),
                 part_num
             LIMIT ?4",
        )
        .bind(&pattern)
        .bind(&prefix)
        .bind(&key)
        .bind(i64::try_from(limit).unwrap_or(25))
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(part_num, name)| CachedPart { part_num, name })
            .collect())
    }

    /// Records a search and every part it returned.
    ///
    /// An empty result is recorded too, with the short expiry — that negative
    /// entry is what stops a typo re-asking the API on every keystroke.
    pub async fn record_search(&self, query: &str, parts: &[CachedPart]) -> anyhow::Result<()> {
        let ttl = self.ttl;
        let key = normalize(query);
        let mut tx = self.pool.begin().await?;

        for part in parts {
            // A part seen only as a search result has no payload yet; do not
            // overwrite one that has been fetched in full.
            sqlx::query(
                "INSERT INTO part_details (part_num, name, name_folded)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(part_num) DO UPDATE SET name = ?2, name_folded = ?3",
            )
            .bind(&part.part_num)
            .bind(&part.name)
            .bind(part.name.to_lowercase())
            .execute(&mut *tx)
            .await?;
        }

        let part_nums: Vec<&str> = parts.iter().map(|p| p.part_num.as_str()).collect();
        let json = serde_json::to_string(&part_nums)?;
        let is_miss = i32::from(parts.is_empty());
        let expiry = if parts.is_empty() {
            format!("+{} minutes", ttl.miss_mins)
        } else {
            format!("+{} days", ttl.search_days)
        };

        sqlx::query(
            "INSERT INTO part_searches (query, part_nums_json, is_miss, fetched_at, expires_at)
             VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, datetime('now', ?4))
             ON CONFLICT(query) DO UPDATE SET
                 part_nums_json = ?2,
                 is_miss = ?3,
                 fetched_at = CURRENT_TIMESTAMP,
                 expires_at = datetime('now', ?4)",
        )
        .bind(&key)
        .bind(&json)
        .bind(is_miss)
        .bind(&expiry)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// A full part record, if one has ever been fetched.
    pub async fn detail(&self, part_num: &str) -> anyhow::Result<Option<RebrickablePart>> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT payload_json FROM part_details WHERE part_num = ?")
                .bind(part_num)
                .fetch_optional(&self.pool)
                .await?;
        let Some((Some(json),)) = row else {
            return Ok(None);
        };
        Ok(serde_json::from_str(&json).ok())
    }

    /// Records a full part record, which never expires.
    pub async fn record_detail(&self, part: &RebrickablePart) -> anyhow::Result<()> {
        let json = serde_json::to_string(part)?;
        sqlx::query(
            "INSERT INTO part_details (part_num, name, name_folded, payload_json, fetched_at)
             VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)
             ON CONFLICT(part_num) DO UPDATE SET
                 name = ?2, name_folded = ?3, payload_json = ?4,
                 fetched_at = CURRENT_TIMESTAMP",
        )
        .bind(&part.part_num)
        .bind(&part.name)
        .bind(part.name.to_lowercase())
        .bind(&json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Drops expired searches. Details are never swept — they are the index.
    ///
    /// The predicate is the exact complement of the one [`Self::lookup`] uses:
    /// an entry is live only while `expires_at` is strictly in the future, so
    /// one that expires exactly now must be swept rather than left behind for
    /// a lookup that will already refuse to read it.
    pub async fn sweep(&self) -> anyhow::Result<u64> {
        let deleted =
            sqlx::query("DELETE FROM part_searches WHERE expires_at <= CURRENT_TIMESTAMP")
                .execute(&self.pool)
                .await?
                .rows_affected();
        if deleted > 0 {
            let _ = sqlx::query("PRAGMA optimize").execute(&self.pool).await;
            tracing::info!("part cache: dropped {deleted} expired search(es)");
        }
        Ok(deleted)
    }

    /// How many parts the index holds, for diagnostics.
    pub async fn len(&self) -> anyhow::Result<i64> {
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM part_details")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An in-memory cache.
    ///
    /// `max_connections(1)` matters: with more, each pooled connection gets
    /// its own private empty `sqlite::memory:` database and the tests silently
    /// see different data.
    async fn cache() -> PartCache {
        with_ttl(Ttl::default()).await
    }

    async fn with_ttl(ttl: Ttl) -> PartCache {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        PartCache::from_pool(pool, ttl).await.unwrap()
    }

    fn part(part_num: &str, name: &str) -> CachedPart {
        CachedPart {
            part_num: part_num.to_string(),
            name: name.to_string(),
        }
    }

    #[tokio::test]
    async fn a_search_is_replayed_from_disk() {
        let cache = cache().await;
        let found = vec![part("3001", "Brick 2 x 4"), part("3020", "Plate 2 x 4")];
        cache.record_search("2x4 brick", &found).await.unwrap();

        assert_eq!(cache.lookup("2x4 brick").await.unwrap(), Some(found));
    }

    #[tokio::test]
    async fn a_query_never_run_is_unknown_rather_than_empty() {
        // The distinction the whole warm path rests on: `None` means "ask",
        // `Some(vec![])` means "we asked, there is nothing".
        let cache = cache().await;
        assert_eq!(cache.lookup("windscreen").await.unwrap(), None);

        cache.record_search("windscreen", &[]).await.unwrap();
        assert_eq!(cache.lookup("windscreen").await.unwrap(), Some(Vec::new()));
    }

    #[tokio::test]
    async fn queries_are_folded_so_spelling_does_not_split_the_cache() {
        let cache = cache().await;
        cache
            .record_search("2x4 brick", &[part("3001", "Brick 2 x 4")])
            .await
            .unwrap();

        for spelling in ["2X4 Brick", "  2x4   brick  ", "2x4 BRICK"] {
            assert!(
                cache.lookup(spelling).await.unwrap().is_some(),
                "{spelling:?} should have hit the same entry"
            );
        }
    }

    #[tokio::test]
    async fn a_part_is_found_by_a_query_nobody_has_run() {
        // The point of the second lookup path. Nobody searched "windscreen",
        // but a search for something else cached one.
        let cache = cache().await;
        cache
            .record_search("cockpit", &[part("2437", "Windscreen 3 x 4 x 1.3")])
            .await
            .unwrap();

        assert!(cache.lookup("windscreen").await.unwrap().is_none());
        let matched = cache.matches("windscreen", 10).await.unwrap();
        assert_eq!(matched, vec![part("2437", "Windscreen 3 x 4 x 1.3")]);
    }

    #[tokio::test]
    async fn matching_prefers_a_part_number_then_a_prefix_then_a_substring() {
        let cache = cache().await;
        cache
            .record_search(
                "seed",
                &[
                    part("9999", "Slope Brick 45 2 x 4"), // substring
                    part("3001", "Brick 2 x 4"),          // prefix
                    part("brick", "Something Else"),      // exact number
                ],
            )
            .await
            .unwrap();

        let matched = cache.matches("brick", 10).await.unwrap();
        let order: Vec<&str> = matched.iter().map(|p| p.part_num.as_str()).collect();
        assert_eq!(order, vec!["brick", "3001", "9999"], "got: {matched:?}");
    }

    #[tokio::test]
    async fn matching_is_capped_and_ignores_an_empty_query() {
        let cache = cache().await;
        let many: Vec<CachedPart> = (0..10)
            .map(|i| part(&format!("p{i}"), &format!("Brick {i}")))
            .collect();
        cache.record_search("seed", &many).await.unwrap();

        assert_eq!(cache.matches("brick", 3).await.unwrap().len(), 3);
        assert!(cache.matches("   ", 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_miss_expires_long_before_a_hit_does() {
        // A zero-length miss TTL, so "expired" is observable without waiting.
        let cache = with_ttl(Ttl {
            search_days: 30,
            miss_mins: 0,
        })
        .await;
        cache.record_search("nothing", &[]).await.unwrap();
        cache
            .record_search("something", &[part("3001", "Brick 2 x 4")])
            .await
            .unwrap();

        // The miss is already stale; the hit is not.
        assert_eq!(cache.lookup("nothing").await.unwrap(), None);
        assert!(cache.lookup("something").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn an_entry_expiring_exactly_now_is_neither_readable_nor_left_behind() {
        // The sweep's predicate has to be the exact complement of the
        // lookup's, or an entry can be unreadable and unswept at once.
        let cache = with_ttl(Ttl {
            search_days: 0,
            miss_mins: 0,
        })
        .await;
        cache
            .record_search("q", &[part("3001", "Brick 2 x 4")])
            .await
            .unwrap();

        assert_eq!(cache.lookup("q").await.unwrap(), None, "unreadable");
        assert_eq!(cache.sweep().await.unwrap(), 1, "and swept");
    }

    #[tokio::test]
    async fn sweeping_drops_expired_searches_but_never_the_index() {
        let cache = with_ttl(Ttl {
            search_days: 0,
            miss_mins: 0,
        })
        .await;
        cache
            .record_search("gone", &[part("3001", "Brick 2 x 4")])
            .await
            .unwrap();

        assert_eq!(cache.sweep().await.unwrap(), 1);
        assert_eq!(cache.lookup("gone").await.unwrap(), None);
        // The part itself survives: it is what makes the next query fast.
        assert_eq!(cache.len().await.unwrap(), 1);
        assert_eq!(cache.matches("brick", 5).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn re_running_a_search_replaces_it_rather_than_duplicating() {
        let cache = cache().await;
        cache
            .record_search("q", &[part("3001", "Brick 2 x 4")])
            .await
            .unwrap();
        cache
            .record_search("q", &[part("3020", "Plate 2 x 4")])
            .await
            .unwrap();

        assert_eq!(
            cache.lookup("q").await.unwrap(),
            Some(vec![part("3020", "Plate 2 x 4")])
        );
        // Both parts stay in the index, though only one is in the search.
        assert_eq!(cache.len().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn a_search_result_never_clobbers_a_fully_fetched_part() {
        let cache = cache().await;
        let full = RebrickablePart {
            part_num: "3001".to_string(),
            name: "Brick 2 x 4".to_string(),
            year_from: 1958,
            year_to: 2026,
            part_url: "https://example.org/3001".to_string(),
            part_img_url: None,
            molds: Vec::new(),
            alternates: Vec::new(),
            print_of: None,
            external_ids: std::collections::HashMap::new(),
        };
        cache.record_detail(&full).await.unwrap();

        // A later search mentions the same part with only a name.
        cache
            .record_search("q", &[part("3001", "Brick 2 x 4")])
            .await
            .unwrap();

        let kept = cache.detail("3001").await.unwrap().expect("payload kept");
        assert_eq!(kept.year_from, 1958);
    }

    #[tokio::test]
    async fn a_part_seen_only_in_a_search_has_no_detail_yet() {
        let cache = cache().await;
        cache
            .record_search("q", &[part("3001", "Brick 2 x 4")])
            .await
            .unwrap();
        assert!(cache.detail("3001").await.unwrap().is_none());
        // But it is still in the index, which is what autocomplete reads.
        assert_eq!(cache.matches("3001", 5).await.unwrap().len(), 1);
    }
}
