# Brickbot PocketBase Feed Syndication Refactoring Plan

This document specifies the architectural and implementation plan to transition **`brickbot`** to use **PocketBase** as its central feed catalog and feed item repository, while retaining **SQLite** strictly for Discord runtime state and notification deduplication.

---

## 1. Architectural Overview & Responsibility Separation

```mermaid
flowchart TD
    subgraph PocketBase ["PocketBase Backend (Remote Source of Truth)"]
        PBFeeds["feeds Collection (Catalog)"]
        PBFeedItems["feed_items Collection (Articles)"]
        PBModeration{"is_hidden == true?"}
    end

    subgraph BrickbotEngine ["Brickbot Runtime Engine"]
        PollLoop["RSS/Atom Polling Loop (src/rss.rs)"]
        PBClient["PocketBase Client (src/pocketbase/feeds.rs)"]
        SetCommand["/set Command (src/commands/set.rs)"]
        
        PollLoop -->|1. Fetch active feeds| PBClient
        PBClient -->|GET /api/collections/feeds/records| PBFeeds
        PollLoop -->|2. Upsert articles| PBClient
        PBClient -->|Query (feed, guid) & Create/Patch| PBFeedItems
        PBModeration -->|Skip Discord announcement| PollLoop
        SetCommand -->|Query articles| PBClient
        PBClient -->|Search is_hidden=false| PBFeedItems
    end

    subgraph LocalSQLite ["Local SQLite (Discord Runtime State Only)"]
        Sightings["feed_sightings (seen item tracking)"]
        Polls["feed_polls (baseline run timestamps)"]
        Pending["feed_pending_announcements (cron digest queue)"]
        ChannelAnn["feed_channel_announcements (digest execution timestamps)"]
        FallbackItems["feed_items (offline fallback cache)"]
        
        PollLoop -->|Record sightings & baseline| Sightings
        PollLoop -->|Record poll timestamps| Polls
        PollLoop -->|Queue scheduled digests| Pending
        PollLoop -->|Track digest runs| ChannelAnn
        SetCommand -.->|Offline fallback| FallbackItems
    end

    subgraph DiscordSurfaces ["Discord Notification Surfaces"]
        Instant["Instant Channel Updates"]
        Digests["Scheduled Digest Messages"]
        
        PollLoop -->|Announce non-hidden| Instant
        Pending -->|Announce non-hidden| Digests
    end
```

### Separation of Responsibilities

| Domain Component | PocketBase Role | SQLite Role |
| :--- | :--- | :--- |
| **Feed Catalog** | **Primary Source of Truth**: Active feeds, URLs, titles, site links, group associations, poll intervals, URL/keyword exclusion rules. | None (supplemented only by static overrides in `config.toml`). |
| **Feed Articles** | **Primary Source of Truth**: Full article repository, canonical GUIDs, links, titles, descriptions, snippets, media types, thumbnails, and moderation status (`is_hidden`, `hidden_reason`, `hidden_at`, `hidden_by`). | **Offline Fallback Cache Only**: Stored during offline operation or fallback. |
| **Discord Sightings** | None. | **Primary**: Tracks item seen state per feed (`feed_sightings`) with 30-day retention pruning. |
| **Baseline Stamp** | None. | **Primary**: Tracks process startup run IDs (`feed_polls.baseline_run`) to prevent flood upon bot restarts. |
| **Scheduled Digests** | None. | **Primary**: Queues channel pending digests (`feed_pending_announcements`) and records last run execution timestamps (`feed_channel_announcements`). |

---

## 2. Configuration (`src/config.rs`) Specification

Extend `PocketBaseConfig` to declare collection overrides for both `feeds` and `feed_items`:

```rust
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct PocketBaseConfig {
    pub url: String,
    pub collection: String, // Existing events collection
    /// Collection holding map locations. Defaults to `locations` when unset.
    #[serde(default)]
    pub locations_collection: Option<String>,
    /// Collection holding syndicated feeds. Defaults to `feeds` when unset.
    #[serde(default)]
    pub feeds_collection: Option<String>,
    /// Collection holding syndicated feed items. Defaults to `feed_items` when unset.
    #[serde(default)]
    pub feed_items_collection: Option<String>,
}

const DEFAULT_LOCATIONS_COLLECTION: &str = "locations";
const DEFAULT_FEEDS_COLLECTION: &str = "feeds";
const DEFAULT_FEED_ITEMS_COLLECTION: &str = "feed_items";

impl PocketBaseConfig {
    #[must_use]
    pub fn locations_collection(&self) -> &str {
        self.locations_collection
            .as_deref()
            .unwrap_or(DEFAULT_LOCATIONS_COLLECTION)
    }

    #[must_use]
    pub fn feeds_collection(&self) -> &str {
        self.feeds_collection
            .as_deref()
            .unwrap_or(DEFAULT_FEEDS_COLLECTION)
    }

    #[must_use]
    pub fn feed_items_collection(&self) -> &str {
        self.feed_items_collection
            .as_deref()
            .unwrap_or(DEFAULT_FEED_ITEMS_COLLECTION)
    }
}
```

### Validation Rules in `Config::validate`:
```rust
if let Some(pb) = &self.pocketbase {
    // ... existing url and collection checks ...
    if pb.feeds_collection.as_ref().is_some_and(String::is_empty) {
        errors.push("PocketBase feeds_collection must not be empty".to_string());
    }
    if pb.feed_items_collection.as_ref().is_some_and(String::is_empty) {
        errors.push("PocketBase feed_items_collection must not be empty".to_string());
    }
}
```

---

## 3. PocketBase Feeds Client Module (`src/pocketbase/feeds.rs`)

Create `src/pocketbase/feeds.rs` (exposed via `pub mod feeds;` in `src/pocketbase.rs`).

### 3.1 Data Structures

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct RemoteFeedConfig {
    pub id: String,
    pub url: String,
    pub title: Option<String>,
    pub site_url: Option<String>,
    pub description: Option<String>,
    pub feed_type: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    pub poll_interval_mins: Option<u64>,
    #[serde(default)]
    pub exclude_keywords: Vec<String>,
    #[serde(default)]
    pub exclude_urls: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpsertFeedItemResult {
    pub id: String,
    pub is_hidden: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct PocketBaseListResponse<T> {
    pub items: Vec<T>,
    pub total_items: Option<u64>,
}
```

### 3.2 Method Signatures & Implementations

#### 1. `fetch_active_feeds`
Fetches all enabled feeds from PocketBase:
```rust
pub async fn fetch_active_feeds(
    http: &dyn crate::http::HttpProvider,
    pb_cfg: &crate::config::PocketBaseConfig,
    limit: u64,
) -> anyhow::Result<Vec<RemoteFeedConfig>> {
    let token = std::env::var("POCKETBASE_IMPERSONATE_AUTH_TOKEN").ok();
    let auth = token.as_ref().map(|t| format!("Bearer {t}"));

    // URL filter: enabled = true, perPage = 500
    let url = format!(
        "{}/api/collections/{}/records?filter=(enabled%3Dtrue)&perPage=500",
        pb_cfg.url.trim_end_matches('/'),
        pb_cfg.feeds_collection()
    );

    let body = http.get_json_with_auth(&url, auth.as_deref(), true, limit).await?;
    let resp: PocketBaseListResponse<RemoteFeedConfig> = serde_json::from_str(&body)?;
    Ok(resp.items)
}
```

#### 2. `upsert_feed_item` (with Moderation Preservation)
**Crucial Invariant**:
- Query existing item by `(feed, guid)`: `GET /api/collections/{feed_items}/records?filter=(feed='{feed_id}'%26%26guid='{encoded_guid}')`.
- If item exists:
  - If `is_hidden == true`:
    - Preserve existing `is_hidden`, `hidden_reason`, `hidden_at`, `hidden_by`.
    - Do not unhide.
    - Return `UpsertFeedItemResult { id: existing_id, is_hidden: true }`.
  - If `is_hidden == false`:
    - Optionally PATCH updated title/description/thumbnail.
    - Return `UpsertFeedItemResult { id: existing_id, is_hidden: false }`.
- If item does not exist:
  - Create via `POST /api/collections/{feed_items}/records`:
    ```json
    {
      "feed": feed_id,
      "guid": entry.id,
      "link": entry.id,
      "title": entry.title,
      "description": entry.description,
      "snippet": truncate(&entry.description, 500),
      "published_at": entry.published.map(|dt| dt.to_rfc3339()),
      "thumbnail": entry.thumbnail,
      "media_type": detect_media_type(&entry.id),
      "is_hidden": false
    }
    ```
  - Return `UpsertFeedItemResult { id: new_id, is_hidden: false }`.

```rust
pub async fn upsert_feed_item(
    http: &dyn crate::http::HttpProvider,
    pb_cfg: &crate::config::PocketBaseConfig,
    limit: u64,
    feed_id: &str,
    entry: &crate::rss::FeedEntry,
) -> anyhow::Result<UpsertFeedItemResult> {
    let token = std::env::var("POCKETBASE_IMPERSONATE_AUTH_TOKEN").ok();
    let auth = token.as_ref().map(|t| format!("Bearer {t}"));

    // 1. Check existing record by (feed, guid)
    let escaped_guid = urlencoding::encode(&entry.id);
    let check_url = format!(
        "{}/api/collections/{}/records?filter=(feed%3D'{}'%26%26guid%3D'{}')&limit=1",
        pb_cfg.url.trim_end_matches('/'),
        pb_cfg.feed_items_collection(),
        feed_id,
        escaped_guid
    );

    let body = http.get_json_with_auth(&check_url, auth.as_deref(), true, limit).await?;
    let check_resp: serde_json::Value = serde_json::from_str(&body)?;

    if let Some(existing) = check_resp.get("items").and_then(|v| v.as_array()).and_then(|a| a.first()) {
        let existing_id = existing["id"].as_str().unwrap_or_default().to_string();
        let is_hidden = existing["is_hidden"].as_bool().unwrap_or(false);

        if is_hidden {
            // CRUCIAL: Preserve moderation barrier, never overwrite or unhide
            return Ok(UpsertFeedItemResult {
                id: existing_id,
                is_hidden: true,
            });
        }

        // Existing and non-hidden: optional update of metadata via PATCH
        let patch_url = format!(
            "{}/api/collections/{}/records/{}",
            pb_cfg.url.trim_end_matches('/'),
            pb_cfg.feed_items_collection(),
            existing_id
        );
        let patch_payload = serde_json::json!({
            "title": entry.title,
            "description": entry.description,
            "thumbnail": entry.thumbnail,
        });
        let _ = http.patch_json_with_auth(&patch_url, auth.as_deref(), &patch_payload, limit).await;

        return Ok(UpsertFeedItemResult {
            id: existing_id,
            is_hidden: false,
        });
    }

    // 2. Insert new record
    let create_url = format!(
        "{}/api/collections/{}/records",
        pb_cfg.url.trim_end_matches('/'),
        pb_cfg.feed_items_collection()
    );
    let media_type = if entry.id.contains("youtube.com") || entry.id.contains("youtu.be") {
        "video"
    } else if entry.id.contains("flickr.com") || entry.id.contains("kuvat.fi") {
        "image"
    } else {
        "article"
    };

    let create_payload = serde_json::json!({
        "feed": feed_id,
        "guid": entry.id,
        "link": entry.id,
        "title": entry.title,
        "description": entry.description,
        "snippet": crate::rss::one_line(&entry.description).chars().take(500).collect::<String>(),
        "published_at": entry.published.map(|dt| dt.to_rfc3339()),
        "thumbnail": entry.thumbnail,
        "media_type": media_type,
        "is_hidden": false
    });

    let resp_str = http.post_json_with_auth(&create_url, auth.as_deref(), &create_payload, limit).await?;
    let resp_val: serde_json::Value = serde_json::from_str(&resp_str)?;
    let id = resp_val["id"].as_str().unwrap_or_default().to_string();

    Ok(UpsertFeedItemResult {
        id,
        is_hidden: false,
    })
}
```

#### 3. `search_feed_items` (for `/set`)
Queries PocketBase `feed_items` collection for items matching set search criteria, ensuring `is_hidden = false` and expanding `feed` relation:
```rust
pub async fn search_feed_items(
    http: &dyn crate::http::HttpProvider,
    pb_cfg: &crate::config::PocketBaseConfig,
    limit: u64,
    set_number: &str,
    set_name: &str,
    set_theme: &str,
) -> anyhow::Result<Vec<crate::db::FeedItem>> {
    let token = std::env::var("POCKETBASE_IMPERSONATE_AUTH_TOKEN").ok();
    let auth = token.as_ref().map(|t| format!("Bearer {t}"));

    let filter = format!(
        "(is_hidden=false && (title ~ '{}' || description ~ '{}' || snippet ~ '{}'))",
        set_number, set_number, set_number
    );
    let encoded_filter = urlencoding::encode(&filter);

    let url = format!(
        "{}/api/collections/{}/records?filter={}&expand=feed&sort=-published_at&limit=20",
        pb_cfg.url.trim_end_matches('/'),
        pb_cfg.feed_items_collection(),
        encoded_filter
    );

    let body = http.get_json_with_auth(&url, auth.as_deref(), false, limit).await?;
    let resp: serde_json::Value = serde_json::from_str(&body)?;

    let mut rows = Vec::new();
    if let Some(items) = resp.get("items").and_then(|v| v.as_array()) {
        for it in items {
            let id = it.get("link")
                .and_then(|v| v.as_str())
                .or_else(|| it.get("guid").and_then(|v| v.as_str()))
                .unwrap_or_default()
                .to_string();
            let source_title = it.get("expand")
                .and_then(|v| v.get("feed"))
                .and_then(|f| f.get("title"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let item_title = it.get("title").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let item_desc = it.get("description").and_then(|v| v.as_str()).unwrap_or_default().to_string();

            rows.push((id, source_title, item_title, item_desc));
        }
    }

    Ok(crate::db::feed::filter_feed_items(rows, set_number, set_name, set_theme))
}
```

---

## 4. Integration into RSS Polling Loop (`src/rss.rs`)

### 4.1 Merging Remote Feeds
In `global_poll_loop`:
1. Check if `config.pocketbase` is configured.
2. If configured, invoke `fetch_active_feeds`.
3. Construct a mapping:
   `let remote_feeds: HashMap<String, RemoteFeedConfig> = ...;` (keyed by feed URL).
4. Remote feed URLs are merged into the set of feeds to poll alongside `feeds_to_poll(&config, &opml_cache)`.

### 4.2 Article Extraction & PocketBase Upsert
When polling `feed_url`:
1. Fetch and parse feed entries (`let entries = feed_entries(&feed);`).
2. If PocketBase is configured and `remote_feeds.get(feed_url)` yields a `feed_id`:
   - For each entry:
     ```rust
     let res = upsert_feed_item(&client, pb_cfg, limit, &remote_feed.id, entry).await;
     if let Ok(res) = res {
         if res.is_hidden {
             hidden_entries.insert(entry.id.clone());
         }
     }
     ```
   - If PocketBase is unconfigured or call fails, fall back to SQLite `insert_feed_item(db, &e.id, ...)` so local cache is preserved.

### 4.3 Preserving SQLite Sighting Records While Suppressing Discord Announcements
In `record_and_route`:
- `record_feed_sightings_in(&mut tx, feed_url, run_id, &ids)` continues to run for ALL items, including hidden ones.
  - **Reason**: Recording sightings in SQLite ensures that if an item is unhidden or bot restarts, it won't trigger an out-of-sequence flood.
- Filter out items present in `hidden_entries` before calling `plan_routing`:
  ```rust
  let visible_fresh: Vec<FeedEntry> = fresh
      .into_iter()
      .filter(|e| !hidden_entries.contains(&e.id))
      .collect();
  let (instant_targets, scheduled) = plan_routing(config, feed_targets, feed_url, &visible_fresh);
  ```
- Result: **Hidden items are never queued in `feed_pending_announcements` and never sent as instant updates.**

---

## 5. Integration into `/set` Command (`src/commands/set.rs` & `src/workflows/search.rs`)

1. Update `set_interaction`:
   ```rust
   pub async fn set_interaction(
       http: &dyn crate::http::HttpProvider,
       db: &SqlitePool,
       pb_cfg: Option<&crate::config::PocketBaseConfig>,
       query: &str,
       locale: &str,
       services: &[String],
       limit: u64,
   ) -> anyhow::Result<SetInteractionResponse>
   ```
2. Article search fallback logic:
   ```rust
   let articles = if let Some(pb) = pb_cfg {
       match crate::pocketbase::feeds::search_feed_items(http, pb, limit, &set.number, &set.name, &set.theme).await {
           Ok(results) => results,
           Err(err) => {
               tracing::warn!("PocketBase feed_items search failed, falling back to SQLite: {err}");
               crate::db::search_feed_items(db, &set.number, &set.name, &set.theme).await.unwrap_or_default()
           }
       }
   } else {
       crate::db::search_feed_items(db, &set.number, &set.name, &set.theme).await.unwrap_or_default()
   };
   ```
3. Update callers in `src/workflows/search.rs`:
   Pass `app_ctx.config.pocketbase.as_ref()`.

---

## 6. Testing Strategy & Mock-Based Unit Tests

All 367 existing tests in `brickbot` must continue to pass cleanly.

### New Test Cases in `src/pocketbase/feeds.rs`:

1. `test_fetch_active_feeds_success`:
   - Mocks `get_json_with_auth` returning PocketBase list JSON with 2 feeds (`enabled = true`).
   - Asserts parsed `RemoteFeedConfig` vector matches expected URLs and exclusion rules.

2. `test_upsert_feed_item_creates_new`:
   - Mocks checking query `GET ... filter=(feed='...' && guid='...')` returning `{"items": []}`.
   - Mocks `POST` returning `{"id": "newitem12345"}`.
   - Asserts `UpsertFeedItemResult { id: "newitem12345", is_hidden: false }`.

3. `test_upsert_feed_item_preserves_is_hidden_true`:
   - Mocks checking query `GET ...` returning an item with `"id": "mod123"`, `"is_hidden": true`, `"hidden_reason": "spam"`.
   - Asserts no `POST` or `PATCH` resets `is_hidden`.
   - Asserts `UpsertFeedItemResult { id: "mod123", is_hidden: true }`.

4. `test_search_feed_items_pocketbase_with_fallback`:
   - Tests successful query returning expanded feed articles and matching through `filter_feed_items`.
   - Tests HTTP 500 / network error fallback to SQLite `search_feed_items`.

5. `test_rss_polling_hidden_items_not_announced`:
   - Unit test simulating `record_and_route` where an item is flagged hidden.
   - Verifies item ID is recorded in SQLite `feed_sightings`, but `updates` and `scheduled` remain empty.

---

## 7. Step-by-Step Implementation Checklist

- [ ] **Step 1: Configuration Updates** (`src/config.rs`)
  - Add `feeds_collection` and `feed_items_collection` to `PocketBaseConfig`.
  - Add accessors and validation in `Config::validate`.
  - Update `config.toml` sample comments if applicable.
- [ ] **Step 2: PocketBase Feeds Module** (`src/pocketbase/feeds.rs`)
  - Create module with `RemoteFeedConfig`, `UpsertFeedItemResult`.
  - Implement `fetch_active_feeds`.
  - Implement `upsert_feed_item` with `(feed, guid)` lookup and `is_hidden` preservation.
  - Implement `search_feed_items` with `expand=feed` and `filter_feed_items` filtering.
  - Expose in `src/pocketbase.rs`.
- [ ] **Step 3: Unit Tests for PocketBase Feeds**
  - Add mock tests using `MockHttpProvider` covering active feed fetching, new creation, `is_hidden` preservation, and search.
- [ ] **Step 4: Integrate `/set` Command Search** (`src/commands/set.rs`, `src/workflows/search.rs`)
  - Update `set_interaction` signature with `Option<&PocketBaseConfig>`.
  - Connect PocketBase search with fallback to SQLite.
  - Update `src/workflows/search.rs` to pass `app_ctx.config.pocketbase.as_ref()`.
- [ ] **Step 5: Integrate RSS Polling Loop** (`src/rss.rs`)
  - Fetch active remote feeds when `config.pocketbase` is active.
  - Upsert items to PocketBase during feed polling.
  - Suppress announcements for `is_hidden == true` items while recording sightings in SQLite.
- [ ] **Step 6: Validation & Verification**
  - Run `cargo test` / `make test-fast` to ensure all existing and new tests pass cleanly.
  - Run `make check` to verify clippy, rustfmt, and treefmt compliance.
