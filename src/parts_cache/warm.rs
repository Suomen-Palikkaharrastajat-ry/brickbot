//! Filling the part cache, and reading through it.
//!
//! Two ways in, and the first matters more than it looks:
//!
//! * [`search`] and [`detail`] are read-through wrappers around the
//!   Rebrickable calls. Point the ordinary `/part` and `/build` lookups at
//!   these and every search a guild runs quietly builds the index that makes
//!   autocomplete instant. Most of the cache's warmth comes from here, not
//!   from anything autocomplete does.
//! * [`spawn_warm`] fills the cache in the background, for autocomplete, which
//!   must never wait for the network.
//!
//! The guards on the background path exist because Discord fires autocomplete
//! on *every keystroke*: without de-duplication, typing `windscreen` would
//! start ten identical searches.

use super::{CachedPart, PartCache, normalize};
use crate::brick::RebrickablePart;
use crate::config::PartsCacheConfig;
use crate::http::HttpProvider;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

/// Below this, a query is too vague to be worth a search.
const MIN_WARM_CHARS: usize = 3;

/// Queries currently being fetched, so a burst of keystrokes causes one
/// request rather than one per character.
static IN_FLIGHT: std::sync::OnceLock<moka::future::Cache<String, ()>> = std::sync::OnceLock::new();

fn in_flight() -> &'static moka::future::Cache<String, ()> {
    IN_FLIGHT.get_or_init(|| {
        moka::future::Cache::builder()
            .max_capacity(1_000)
            // Long enough to cover a request, short enough that a failed one
            // does not block a retry for long.
            .time_to_live(Duration::from_secs(30))
            .build()
    })
}

/// Caps concurrent background warms, the way `render_slots` caps renders.
static WARM_SLOTS: std::sync::OnceLock<Semaphore> = std::sync::OnceLock::new();

fn warm_slots(permits: usize) -> &'static Semaphore {
    WARM_SLOTS.get_or_init(|| Semaphore::new(permits.max(1)))
}

/// Searches for parts, answering from the cache whenever it can.
///
/// A cached miss is honoured as an answer: if the API said there was nothing
/// an hour ago, asking again on every keystroke helps nobody.
pub async fn search(
    cache: Option<&PartCache>,
    http: &dyn HttpProvider,
    query: &str,
    limit: u64,
) -> anyhow::Result<Vec<CachedPart>> {
    if let Some(cache) = cache {
        match cache.lookup(query).await {
            Ok(Some(hit)) => return Ok(hit),
            Ok(None) => {}
            // A broken cache must not break a lookup that would otherwise work.
            Err(e) => tracing::warn!("part cache lookup failed for {query:?}: {e}"),
        }
    }

    let found: Vec<CachedPart> = crate::brick::search_parts(http, query, limit)
        .await?
        .into_iter()
        .map(CachedPart::from)
        .collect();

    if let Some(cache) = cache {
        if let Err(e) = cache.record_search(query, &found).await {
            tracing::warn!("part cache write failed for {query:?}: {e}");
        }
    }
    Ok(found)
}

/// Fetches one part, answering from the cache whenever it can.
///
/// Part records never expire: a 2x4 brick's name does not change.
pub async fn detail(
    cache: Option<&PartCache>,
    http: &dyn HttpProvider,
    part_num: &str,
    limit: u64,
) -> anyhow::Result<RebrickablePart> {
    if let Some(cache) = cache {
        match cache.detail(part_num).await {
            Ok(Some(hit)) => return Ok(hit),
            Ok(None) => {}
            Err(e) => tracing::warn!("part cache detail failed for {part_num:?}: {e}"),
        }
    }

    let part = crate::brick::fetch_part(http, part_num, limit).await?;
    if let Some(cache) = cache {
        if let Err(e) = cache.record_detail(&part).await {
            tracing::warn!("part cache detail write failed for {part_num:?}: {e}");
        }
    }
    Ok(part)
}

/// Whether a query is worth warming the cache for.
///
/// Pure, so the guard is testable without a network or a clock.
#[must_use]
pub fn worth_warming(query: &str) -> bool {
    normalize(query).chars().count() >= MIN_WARM_CHARS
}

/// Fills the cache for a query, in the background.
///
/// Returns immediately. Autocomplete calls this and answers from whatever it
/// already has: the keystroke that misses is the one that pays nothing, and
/// the next one is fast.
pub fn spawn_warm(
    cache: Arc<PartCache>,
    http: Arc<dyn HttpProvider>,
    query: String,
    cfg: &PartsCacheConfig,
    body_limit: u64,
) {
    if !worth_warming(&query) {
        return;
    }

    // Take what the task needs before spawning it, so the config does not have
    // to outlive the caller.
    let concurrency = cfg.warm_concurrency;
    let timeout = Duration::from_secs(cfg.warm_timeout_secs);

    tokio::spawn(async move {
        let key = normalize(&query);

        // One fetch per query, however fast someone types.
        if in_flight().get(&key).await.is_some() {
            return;
        }
        in_flight().insert(key.clone(), ()).await;

        let _permit = warm_slots(concurrency).acquire().await.ok();

        // `src/http.rs` has no timeout of its own, so a hung request would
        // otherwise hold a slot until the process ends.
        let fetch = search(Some(cache.as_ref()), http.as_ref(), &query, body_limit);
        match tokio::time::timeout(timeout, fetch).await {
            Ok(Ok(found)) => {
                tracing::debug!("part cache: warmed {query:?} with {} part(s)", found.len());
            }
            Ok(Err(e)) => tracing::debug!("part cache: could not warm {query:?}: {e}"),
            Err(_) => tracing::debug!("part cache: warming {query:?} timed out"),
        }

        in_flight().invalidate(&key).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_too_short_to_mean_anything_is_not_warmed() {
        // Autocomplete fires from the first character; searching for "w" would
        // spend a request on nothing.
        assert!(!worth_warming("w"));
        assert!(!worth_warming("wi"));
        assert!(!worth_warming("  "));
        assert!(worth_warming("win"));
        assert!(worth_warming("windscreen"));
    }

    #[test]
    fn warming_judges_the_normalised_query() {
        // Padding is not length.
        assert!(!worth_warming("  w  "));
        assert!(worth_warming("  win  "));
    }
}
