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

### Bypassing the Cache
For operations where fresh data is strictly required, the client provides un-cached variants like `get_bytes_no_cache` and `get_text_no_cache`. These methods execute the network request directly and then insert the fresh result into the cache.

## Testability
The `HttpClient` is abstracted behind the `HttpProvider` trait (`automock` via `mockall`). This allows for seamless dependency injection during unit testing, meaning tests for RSS polling, event syncing, or LEGO API parsing do not require a live network connection.
