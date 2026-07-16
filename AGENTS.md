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
- **Localization (i18n)**: All new user-facing strings must be localized using the `rust_i18n::t!` macro. Ensure strings are added to both `locales/en-US.yml` and `locales/fi-FI.yml` to keep them up to date.

## Architecture & Project Structure

The project has been structured to handle multiple asynchronous background tasks while providing interactive commands, supporting multi-guild configurations.

- `src/main.rs`: Application entry point. Handles setup of dependencies, DB pools, custom event/message handler routing, and spawning polling loops.
- `src/config.rs`: Multi-guild TOML configuration structures mapping `config.toml`. Includes flexible command alias configurations.
- `src/http.rs`: Centralized `HttpClient` wrapper. Consolidates outgoing `reqwest` calls, implements structured `tracing` logging, and provides a global `moka` in-memory cache for API responses.
- `src/commands/`: "One command per module" directory containing `set.rs`, `part.rs`, and `events.rs`. **Agents MUST keep the corresponding documentation files in `docs/interactions/*.md` and `docs/commands/*.md` up-to-date when altering command logic or adding new commands.**
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

## Recent Context & Known Gotchas

- **Nix Build Fixes**: The project recently separated `default.nix` and `flake.nix` to fix a `src = ./.;` issue that caused rebuilding on any unrelated file changes.
- **Missing Module Errors in Nix**: When adding new source files, they **must** be tracked by `git` (e.g., `git add src/new_file.rs`), otherwise the Nix build process will fail with `file not found for module` errors.
- **Database Initialization**: If encountering `database file could not be opened`, ensure `mkdir -p data` has been executed. SQLx can create the `.db` file, but not the parent directory.
