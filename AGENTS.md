# Brickbot Development Guide

This document contains detailed information intended for coding agents, LLMs, and developers working on the `brickbot` (Brickbot) codebase. For installation and usage instructions, refer to `README.md`.

## Overview

`brickbot` is a statically linked Rust-based Discord bot. It connects to Discord using Serenity, polls RSS and OPML feeds, parses iCalendar (`.ics`) events, fetches LEGO set/part data, stores state in an SQLite database, and posts updates to Discord channels across multiple guilds.

## Technology Stack

- **Rust**: Core programming language (Edition 2021).
- **Serenity (`0.12`)**: Discord API wrapper (using `rustls_backend`).
- **Tokio (`1`)**: Asynchronous runtime for concurrent tasks and polling loops.
- **Reqwest (`0.12`)**: HTTP client for fetching RSS feeds, iCal data, and LEGO APIs.
- **Moka (`0.12`)**: Fast, concurrent in-memory caching.
- **RSS (`2.0`) & OPML (`1.1`)**: Feed parsing libraries.
- **SQLx (`0.8`)**: Async SQLite database interaction, using `runtime-tokio-rustls`.
- **rust-i18n (`3`)**: Lightweight macro-based localization system for `en-US` and `fi-FI` support.
- **Nix**: Used for deterministic environments (`devenv`), dependency management, and generating statically-linked MUSL binaries.

## Development Methodology

- **Agent Maintainability & Testability**: To keep this project highly maintainable by AI coding agents, the core business logic (formatting, algorithms, decision making) **MUST** be strictly decoupled from external API surfaces (like `serenity` Discord integrations or `reqwest`).
  - Extract message building and complex logic into pure, highly unit-testable functions (e.g., `build_set_message`).
  - Target 100% test coverage by extensively covering these pure functions.
  - Utilize Traits or Dependency Injection to mock database interfaces and HTTP clients when testing the outer orchestrating layers.
- **Red / Green TDD Requirement**: All new feature development or bug fixes must follow the Test-Driven Development (TDD) cycle. First, write a failing test (Red), implement the minimum code required to make it pass (Green), and finally refactor while keeping the tests passing.
- **Test Coverage**: Run `make coverage` to generate HTML test coverage reports using `cargo-llvm-cov` or `cargo-tarpaulin`. Check `lcov.info` or HTML output.
- **Formatting & Checks**: Run `make format check` before committing.
- **Localization (i18n)**: All new user-facing strings must be localized using the `rust_i18n::t!` macro. Ensure strings are added to both `locales/en-US.yml` and `locales/fi-FI.yml` to keep them up to date. This includes Discord modal titles, input labels and validation errors — the tests in `src/i18n.rs` fail on any key, placeholder or blank-value drift between the two bundles. Modal titles and input labels are capped at 45 characters by Discord, so keep translations short.

## Architecture & Project Structure

The project has been structured to handle multiple asynchronous background tasks while providing interactive commands, supporting multi-guild configurations.

- `src/main.rs`: Application entry point. Handles setup of dependencies, DB pools, custom event/message handler routing, and spawning polling loops.
- `src/config.rs`: Multi-guild TOML configuration structures mapping `config.toml`. Includes flexible command alias configurations.
- `src/http.rs`: Centralized `HttpClient` wrapper. Consolidates outgoing `reqwest` calls, implements structured `tracing` logging, and provides a global `moka` in-memory cache for API responses.
- `src/commands/`: "One command per module" directory containing `set.rs`, `part.rs`, `events.rs`, `locations.rs`, and `build.rs`. **Agents MUST keep the corresponding documentation files in `docs/interactions/*.md` and `docs/commands/*.md` up-to-date when altering command logic or adding new commands.**
- `src/workflows/`: Multi-step wizard logic backing the interactive commands (`events/`, `locations/`, `search.rs`, `ambient.rs`). `WizardInteraction` in `src/workflows/mod.rs` is shared by every wizard.
- `src/zulip/moderation.rs`: `ModerationKind` resolves everything that differs between the events and locations moderation flows (target collection, localization keys, public URL) from the Zulip topic prefix, keeping `process_zulip_message` single-pathed.
- `src/rss.rs`: Polling logic and parsing for feeds.
- `flake.nix` & `devenv.nix`: Nix environment files configuring the `devShell`.
- `default.nix`: Nix build configuration decoupled from `flake.nix` to prevent "dirty tree" rebuilds.
- `data/bot.db`: SQLite database file. Ensure the `data` directory exists locally to avoid "database file could not be opened" startup errors.

**Architectural Documentation:**
Agents and developers should consult the following living documents when modifying core systems:
- **Database**: [`docs/architecture/database.md`](docs/architecture/database.md)
- **Network & Caching**: [`docs/architecture/network_and_caching.md`](docs/architecture/network_and_caching.md)
- **Events Sync & Workflow**: [`docs/events.md`](docs/events.md)
- **Ambient Assistant**: [`docs/interactions/README.md`](docs/interactions/README.md)
- **LDraw Rendering & Studio Export**: [`docs/ADR-003-LDraw-Rendering-And-Studio-Export.md`](docs/ADR-003-LDraw-Rendering-And-Studio-Export.md)


## Build and Environment (Nix)

The project relies strictly on **Nix** for a reproducible environment.

- **Development Shell**: `devenv shell` drops you into a shell with `rustc`, `cargo`, and `sqlite` configured correctly for MUSL targets.
- **Production Build**: `nix build` creates a statically linked MUSL binary located at `result/bin/brickbot`.

## Key Patterns & Workflows

1.  **Configuration**: Driven by `config.toml`, supporting multiple prefixes, command aliases, and multiple server/guild setups.
2.  **Database Access**: Uses `sqlx::SqlitePool`. Data tracking ensures items (RSS entries, event advertisements) are not double-posted. The SQLite database strictly requires the `data/` directory to exist.
3.  **Polling Loops**: Background `tokio` tasks poll external resources (`RSS`/`OPML`) at intervals defined in `config.toml`.
4.  **Network & Caching**: All outbound HTTP traffic should route through `HttpClient` (`src/http.rs`) to leverage the centralized `moka` cache, preventing redundant network requests.
5.  **Message Handling**: The bot bypasses standard framework macros in favor of a raw `EventHandler` in `main.rs` to allow dynamic prefix and alias matching defined in the configuration.

## The `/build` Command (BrickTalk)

Collaborative LEGO building in a Discord thread, driven by a hand-written recursive-descent parser for a controlled natural language — no LLM involved, so interpretation is reproducible. Off by default (`[commands.build]` in `config.toml`, per-guild override supported). Localized as `/build` (en-US) or `/rakenna` (fi-FI); the BrickTalk language itself is **not** translated — verbs, colours and units stay English in every locale.

**Pipeline** (pure/unit-tested except the Discord shell at the end):
`src/build/dsl/block.rs` (lifts fenced drawings out verbatim *before* anything else reads the message) → `src/build/dsl/{lexer,parser,ast}.rs` (tokenize + grammar, including the silence policy) → `src/build/lower.rs` (resolves references/locations against the model, emits `Op`s, triggers part lookups) → `src/build/expand.rs` (walls/towers/rows/floors/boxes/stairs → individual placements) and `src/build/grid.rs` (a drawing's cells → individual placements) → `src/build/ops.rs` (`History`/`Op` append-only log, `apply`) → `src/build/dispatch.rs::process_message` (the single pure entry point; returns an `Outcome` of ops, replies, reactions, render/export/finish flags) → `src/build/session.rs` (the only part that touches Discord: thread creation, persistence, the debounced render loop, buttons and the part-picker dropdown).

**Silence policy** (the most important behavior): a message is parsed as a command only if its first word is a known verb or a question word; everything else — ordinary chat — is ignored with no reply, reaction or render, so a build thread stays usable for talking. A known-verb sentence that fails to parse *is* reported (the builder clearly meant something); a question-word sentence that can't be answered stays silent, because question forms are shaped identically to idle chat ("where is everyone?" vs. "where is tower?"). Discord block-quote lines (`> …`, `>>> …` to end of message) are stripped in `strip_discord_noise` before parsing, so quoting the bot's own instructions back at it — e.g. "did this work?" — never re-runs them.

**Collaborative state**: sessions are keyed by thread channel id, not by owner, in `build_sessions`/`build_revisions` (`src/db/build.rs`) — every guild member edits the same model. Each accepted command becomes a revision in the append-only log; the read-interpret-write cycle is guarded by a per-session lock and committed in one transaction. `Undo alice's last edit.` resolves chat names to Discord user ids via `dispatch::Builders` (exact name → user id → unambiguous prefix).

**Rendering**: bursts of commands debounce into one render (`render_debounce_ms`, leading+trailing edge) via `src/ldraw/` (part geometry) and `src/render/` (software rasterizer) to PNG. Falls back to plain boxes, with a one-time notice, if the LDraw part library isn't installed (`make fetch-ldraw`; see [ADR-003](docs/ADR-003-LDraw-Rendering-And-Studio-Export.md)).

**Export**: the `[Export .io]` button or `Export.` sentence produces a BrickLink Studio `.io` file via `src/build/export/`, containing part references only, never geometry.

**BrickTalk language**: verbs — `add`/`put`/`place`, `move`/`shift`, `raise`/`lower`, `rotate`/`turn`, `remove`/`delete`, `replace`/`swap`, `paint`/`colour`, `build`/`make`, `duplicate`/`mirror`, `select`, `group`/`call`/`name`, `undo`/`redo`/`revert`, `count`/`find`/`show`, `export`/`render`/`help`/`finish`. Parts: `[count] [colour] [size] [kind]` in any order, or a bare 4-digit part number. Units: 1 stud = 20 LDU, 1 plate = 8, 1 brick = 24; a bare count is studs horizontally but bricks vertically. Locations: `at origin`, `at x y z`, `at <landmark>`, `on`/`on top of`, `under`, `<direction> of` — with none given, a part stacks on whatever was last touched. Full grammar and examples: `docs/commands/build.md`.

**Part and colour picking**: `src/parts_cache/` is a **second SQLite database** (`data/cache.db`, own pool, own `migrations_cache/` — pointing it at `./migrations` would build the whole bot schema in it) holding every part lookup the bot has made. Deliberately separate from `bot.db`: all of it is derived and disposable. Two lookup paths — exact query replay, and prefix/substring over `part_details.name`, which answers queries nobody has typed before. Tiered expiry: details never, searches 30 days, empty results 1 hour. `parts_cache::warm::{search,detail}` are read-through wrappers that **all** part lookups go through (`/part`, the `/build` picker, ambient), so ordinary use warms the index. `Interaction::Autocomplete` (`main.rs`) answers `/part`'s argument from the compiled-in catalogue plus the cache and **never awaits the network**, spawning a guarded background warm instead (de-dup, min 3 chars, timeout, semaphore — `src/http.rs` has no timeout of its own). Colour suggestions (`colors::suggest`, Levenshtein over the 74-entry palette) are always local. `REBRICKABLE_API_KEY` is env-only; without it lookups fail and autocomplete degrades to the catalogue.

**Editing a legend**: a modal holds only text inputs, so `/build-legend` (autocompleted part and colour) and the `[Insert…]` picker edit the draft from outside the form and re-post the preview. `[Insert…]` is one flow in **two steps** — colour then part — because a colour alone is not a legend line; the colour rides between steps as its LDraw code in the custom id, so nothing is stored. It is **the only paginated list in the bot** (everywhere else truncates at 25), over the 74 colours and the 38 catalogue parts; `page_count`/`page_bounds` are unit-tested. Drafts are found with `db::find_latest_workflow_session` (owner-scoped) and closed with `consume_workflow_session` on apply or discard — without which `[Draw…]` would open a second one each press.

**Private drafts**: `[Draw…]` on a render opens a four-field modal (view/legend/grid/location) whose submission previews **ephemerally** and writes nothing — `src/build/draft.rs` assembles the fields into ordinary BrickTalk and runs `process_message` against a clone of the history with `persist` skipped, so preview and apply cannot diverge. The form opens prefilled from `textview` (editing what exists is the easiest part-picker). `[Apply]` refuses if the session's revision moved since the preview. Discord constraints that shape it: a modal holds only text inputs (no pickers inside), a modal cannot open from a modal submit (hence the `[Edit]` button bounce), and the 3s ACK means text first then the PNG edited in, guarded by `preview_seq`. Drafts live in owner-scoped `workflow_sessions`; no schema change.

**Drawings**: a header sentence plus a fenced block of legend and grid — `Build from top at origin:` then ```` ``` ````, `b = blue 1x1 brick`, a blank line, `..bb..`. One cell = one stud; a symbol repeated over a rectangle matching its footprint is **one** part (greedy from the top left, footprint tried as drawn then turned a quarter, so legend spelling decides the tiling). Six views (`top`/`bottom`/`north`/`south`/`east`/`west`); blank lines separate layers. `Show top view.` (`src/build/textview.rs`) draws the model back **in the same syntax**, so the reply can be pasted in to rebuild it — the round trip is the feature's main correctness property. Capture has to precede `strip_discord_noise` and `split_sentences`, both of which would destroy a grid (`.` ends a sentence, and the URL pass rejoins lines on single spaces). A parked sentence carries its fence via `Sentence::replay_text`, which caps a drawing at one unknown legend part.

**Per-command help**: `Help.` shows the general overview; `Help <verb>.` (e.g. `Help select.`) shows that verb's own syntax and examples — `Verb::help_key` (`src/build/dsl/lexer.rs`) maps the 24 documented verbs to `command.build.help_topic.*` locale keys, synonyms resolve to their canonical verb first, and an unrecognised or alias-only topic (`Attach`/`Fill`/`Repeat`/`Measure`) falls back to the general text rather than erroring.

**Known bug** (documented by an `#[ignore]`d regression test in `src/build/dispatch.rs`): within a *single* multi-sentence message, `Outcome::ops` is append-only, so an undo sandwiched between two commands (e.g. `Add red. Add blue. Undo. Add yellow.`) persists a log the interpreter never actually held — the superseded operation survives in what gets written. Sending the same steps as separate messages is unaffected and correct.

**Grammar iteration**: `--build-debug-log <path>` (CLI flag, off by default) appends a JSONL line — timestamp, session, author, locale, raw message, `classification` (`ignored`/`silent`/`ok`/`problem`), reactions, replies, ops applied — for every message any build thread receives, via `src/build/debug_log.rs`. `problem` lines are known-verb sentences that failed to parse; `ignored` lines are what the silence policy swallowed as chat. Both are the ones worth reading when deciding what to extend in `src/build/dsl/`. Details: [`docs/commands/build.md`](docs/commands/build.md#debugging--grammar-iteration).

- **Docs**: [`docs/interactions/build.md`](docs/interactions/build.md) (trigger, silence policy, internals) and [`docs/commands/build.md`](docs/commands/build.md) (full BrickTalk grammar, config, example session) are the source of truth — keep them in sync with `src/build/*` changes.

## No-LLM Stance (ADR-001 and BrickTalk)

Nothing in this codebase calls an external LLM. [ADR-001](docs/ADR-001-Ambient-Conversational-Discord-Assistant.md)'s ambient assistant (`src/ambient.rs`) is `AhoCorasick` keyword matching plus a regex for set numbers, scored into Low/Medium/High confidence — explicitly chosen for speed and privacy over a model. BrickTalk (`/build`) is a hand-written recursive-descent parser for the same reason: deterministic, reproducible interpretation. Any future NLP work for `/build` should stay in that spirit — richer grammar/synonyms/fuzzy matching rather than an LLM in the interpretation loop — unless the user decides otherwise.

## Recent Context & Known Gotchas

- **Nix Build Fixes**: The project recently separated `default.nix` and `flake.nix` to fix a `src = ./.;` issue that caused rebuilding on any unrelated file changes.
- **Missing Module Errors in Nix**: When adding new source files, they **must** be tracked by `git` (e.g., `git add src/new_file.rs`), otherwise the Nix build process will fail with `file not found for module` errors.
- **Database Initialization**: If encountering `database file could not be opened`, ensure `mkdir -p data` has been executed. SQLx can create the `.db` file, but not the parent directory.
