# Brickbot API Usage & Ecosystem Review

This document provides a comprehensive review of the external LEGO ecosystem APIs and endpoints used by Brickbot.

## Overview of Strategy
The bot adopts a highly optimized API separation-of-concerns:
- **Brickset**: Used exclusively for LEGO **Sets**.
- **Rebrickable**: Used exclusively for LEGO **Parts**.
- **BrickLink / LEGO.com**: Used only as outbound links (no direct API consumption).

This split is intentional and optimal. Brickset is the community gold standard for set metadata (including pricing, subthemes, and ratings), while Rebrickable is the premier database for part inventories, molds, and external ID cross-referencing. 

All API outbound calls are routed through `src/http.rs`, which implements strict bounding (memory limits) and high-performance in-memory caching via `moka` to avoid redundant external network requests.

---

## 1. Brickset API
**File:** `src/brick.rs`
**Endpoints Used:** `https://brickset.com/api/v3.asmx/getSets`
- **Purpose**: Used for resolving set numbers (`fetch_set`) and searching by keyword (`search_sets`).
- **Methodology**: 
  - Standard POST Form request (`apiKey`, `userHash`, `params`).
  - **Heuristics**: When querying a specific set (e.g., `42083`), the bot automatically appends `-1` (to `42083-1`) if no hyphen is present. This drastically improves direct match accuracy since Brickset typically indexes retail sets with a `-1` suffix.
- **Optimization Status**: **Optimal**. The caching system perfectly deduplicates identical set queries.

## 2. Rebrickable API
**File:** `src/brick.rs`
**Endpoints Used:** 
- `https://rebrickable.com/api/v3/lego/parts/{part_num}/` (Direct fetch)
- `https://rebrickable.com/api/v3/lego/parts/?search={query}` (Keyword search)
- **Purpose**: Part metadata resolution.
- **Methodology**:
  - Requires standard `key {api_key}` Authorization header.
  - Returns incredibly rich metadata, including a dictionary of `external_ids`.
- **Optimization Status**: **Optimal, with one minor improvement possible**. The bot brilliantly parses the `external_ids` map to generate accurate BrickLink URLs (falling back to Rebrickable's design ID if missing). However, it does not currently use the `external_ids` map for generating the LEGO Pick-a-Brick URLs.

## 3. BrickLink API
**File:** `src/links.rs`
- **Purpose**: Generating outbound links to the BrickLink catalog.
- **Methodology**: The bot dynamically builds URLs (`https://www.bricklink.com/v2/catalog/catalogitem.page?S={set_id}`).
- **Optimization Status**: **Optimal**. Brickbot specifically avoids the BrickLink API for data ingestion. The BrickLink API requires strict OAuth 1.0a signatures, enforces a brutal 5,000-request-per-day limit per IP/Token pair, and its search endpoints are inferior to Rebrickable and Brickset. Linking to their catalog is the correct approach.

## 4. LEGO.com (Official)
**File:** `src/links.rs`
- **Purpose**: Generating outbound links for retail search and "Pick a Brick".
- **Methodology**: The bot dynamically builds locale-specific URLs (`https://www.lego.com/fi-fi/search?q={query}`).
- **Optimization Status**: **Optimal**. LEGO does not offer a stable, public official API. Scraping or querying their internal GraphQL endpoints often results in aggressive bot-blocking or Cloudflare captchas. Relying on simple URL redirection is the safest method.
