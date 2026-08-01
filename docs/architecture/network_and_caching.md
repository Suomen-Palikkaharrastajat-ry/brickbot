# Network & Caching Architecture

Brickbot centralizes all outbound HTTP traffic through a custom wrapper to ensure consistent logging, error handling, and memory-efficient caching.

## The `HttpClient` Wrapper (`src/http.rs`)

Agents and developers **MUST NOT** use raw `reqwest::Client` directly in business logic. Instead, they should utilize the `HttpClient` struct or the `HttpProvider` trait provided in `src/http.rs`.

### Why a Centralized Client?
1. **Consistent Tracing**: Every outbound request logs exactly what URL is being fetched, along with its success or failure status and standard error messages, integrating with the `tracing` framework.
2. **Global Connection Pool**: Reusing a single underlying `reqwest::Client` maximizes HTTP connection reuse.
3. **In-Memory Caching**: Implements a concurrent cache to automatically deduplicate redundant network requests.

## Caching Strategy (`moka`)

To reduce bandwidth, rate limiting issues, and latency (e.g., when multiple polling loops request the same RSS feed or API data), Brickbot utilizes `moka` for fast, concurrent, in-memory caching.

### Cache Configuration
The `moka` cache is lazily initialized via a `OnceLock` and is configured via environment variables (with sensible defaults):
- `CACHE_MAX_MEMORY_BYTES`: Defines the maximum memory capacity (Default: 50MB). The `weigher` function calculates size based on the byte length of the HTTP response.
- `CACHE_TTL_SECS`: Defines how long an item lives in the cache before expiring (Default: 10 minutes).

### Supported Operations
The `HttpClient` currently caches the following operations:
- `get_bytes`: Fetches raw bytes (e.g., for downloading event cover images or OPML feeds).
- `get_text`: Fetches UTF-8 strings (e.g., for RSS feeds).
- `get_json_with_auth`: Fetches and deserializes JSON payloads using a Bearer token.
- `post_form_json`: Submits a form payload and caches the response.

> **Stale:** the method names in this section predate the current `HttpProvider` trait — see `src/http.rs` for the real surface. The behaviour described is still accurate.

## The Sticky Part Cache (`src/parts_cache/`)

The `moka` cache above is deliberately short-lived, in-memory, and keyed on the *request*: its key includes a `DefaultHasher` of the auth header, which is randomly seeded per process. It cannot be persisted, and it is empty after every restart.

Part lookups need the opposite. Discord fires slash-command autocomplete on **every keystroke** with a three-second deadline and no defer, so the network can never be on that path — and we cannot ship a comprehensive LEGO catalogue. So part lookups are cached durably, keyed on the *query*, in a second SQLite database.

### A separate database

`data/cache.db`, its own pool, its own `migrations_cache/` directory applied with an explicit `sqlx::migrate!("./migrations_cache")`. Pointing the cache pool at `./migrations` would create the entire bot schema inside the cache file.

It is separate from `data/bot.db` because everything in it is derived and disposable: deleting the file is a clean rebuild, backups of real state stay small, and it can run `journal_mode=WAL` with `synchronous=NORMAL`, which would be the wrong trade for session data.

### Two tables, two volatilities

| Table | Key | Expiry |
|---|---|---|
| `part_details` | `part_num` | **never** — a 2x4 brick's number and name do not change |
| `part_searches` | normalised query | 30 days for a hit, **1 hour** when the search found nothing |

The negative entry matters: without it a typo would re-ask the API on every keystroke.

### Two lookup paths

- `PartCache::lookup` replays a query someone has run before.
- `PartCache::matches` prefix- and substring-matches over `part_details.name`, answering queries *nobody has typed before* as long as the part has been seen. Typing `wind` finds `Windscreen 3 x 4 x 1.3` because an unrelated earlier search cached it, so the index ends up better than the sum of the queries put into it.

### Filling it

`parts_cache::warm::{search, detail}` are read-through wrappers around the Rebrickable calls, and **every** part lookup goes through them — `/part`, the `/build` part picker, and the ambient assistant. Ordinary use of the bot is therefore what warms the index that makes autocomplete good.

`parts_cache::warm::spawn_warm` fills it in the background for autocomplete, which answers from what it already has. Three guards, because autocomplete fires per keystroke: in-flight de-duplication (one fetch per query however fast someone types), a three-character minimum, and a timeout plus a concurrency semaphore. The timeout is not optional — `src/http.rs` has no timeout, retry or rate limiting of its own.

Expired searches are swept daily alongside `cleanup_expired_rows`; part records are never swept.

### Bypassing the Cache
For operations where fresh data is strictly required, the client provides un-cached variants like `get_bytes_no_cache` and `get_text_no_cache`. These methods execute the network request directly and then insert the fresh result into the cache.

## Testability
The `HttpClient` is abstracted behind the `HttpProvider` trait (`automock` via `mockall`). This allows for seamless dependency injection during unit testing, meaning tests for RSS polling, event syncing, or LEGO API parsing do not require a live network connection.
