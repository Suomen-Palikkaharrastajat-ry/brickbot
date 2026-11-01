# Operator Runbook

## Required Discord Intents & Permissions

When inviting the bot or configuring the Discord Developer Portal, the intents and scopes depend on which features are enabled in `config.toml`:

### Gateway Intents
- **Standard Unprivileged Intents (Always Requested)**:
  - `GatewayIntents::GUILDS`: Required for basic bot operation, slash command registration, and guild metadata caching.
  - `GatewayIntents::DIRECT_MESSAGES`: Required for handling Zulip moderation event replies via direct messages.
- **Conditional / Privileged Intents**:
  - `GatewayIntents::MESSAGE_CONTENT` (Privileged): Required **only** when the ambient assistant (`[ambient]` / `[guilds.ambient]`) or collaborative `/build` (`[commands.build]` / `[guilds.commands.build]`) is enabled in at least one configured server.
  - `GatewayIntents::GUILD_MESSAGES`: Required **only** when ambient assistant or collaborative `/build` is enabled, allowing the bot to receive message events in guild channels and build threads.

> [!NOTE]
> If neither ambient assistance nor collaborative `/build` is enabled for any configured guild (or globally when no guilds are specified), `brickbot` automatically omits both `GatewayIntents::MESSAGE_CONTENT` and `GatewayIntents::GUILD_MESSAGES` at connection time and runs in unprivileged intent mode. In this mode, operators do not need to enable the privileged Message Content intent in the Discord Developer Portal.

### Scopes & Bot Permissions
- `bot`
- `applications.commands` (for slash commands like `/events`)
- **Permissions**:
  - `View Channels` (Ambient assistant & core)
  - `Send Messages` (Replying to users)
  - `Embed Links` (Rich embeds for LEGO sets)
  - `Manage Events` & `Create Events` (PocketBase event sync)

## Configuration & Secret Provisioning

Secrets are provided via the `.env` file (e.g., `DISCORD_TOKEN`, `DATABASE_URL`, `ZULIP_API_KEY`, `POCKETBASE_IMPERSONATE_AUTH_TOKEN`).
All other feature configurations (intervals, resource limits, integrations) live in `config.toml`.

- To validate your configuration without starting the bot or connecting to Discord, run:
  ```bash
  cargo run --bin brickbot -- --check-config --config config.toml
  ```

## LDraw Part Library

The `/build` command renders models from the LDraw part library. The library is **not** bundled with the binary and is **not** downloaded during a render; install it once:

```bash
make fetch-ldraw            # or: cargo run --bin brickbot -- --fetch-ldraw
```

- It is installed into `[ldraw] data_dir` (default `data/ldraw`), from `[ldraw] library_url`.
- Afterwards `data/ldraw/parts/`, `data/ldraw/p/` and `data/ldraw/.ldraw-sync.json` should exist. The lockfile records what was installed.
- Files marked *Not redistributable* are skipped and never written to disk; the count appears in the log line and in the lockfile.
- **Until the library is installed `/build` still works**, rendering every part as a plain box and saying so once per session. Nothing else is affected.
- Re-run the command to update the library after a new LDraw release.

See [ADR-003](ADR-003-LDraw-Rendering-And-Studio-Export.md) for why it is installed rather than bundled.

## Database Backup, Restore, and Migration

The bot uses SQLite (`data/bot.db`).

- **Backup**: Copy `data/bot.db` and any `-wal`/`-shm` files to a safe location while the bot is stopped, or use the `.backup` command in `sqlite3` CLI.
- **Restore**: Stop the bot and replace the `data/bot.db` file.
- **Migration**: Handled automatically on startup via `sqlx::migrate!()`. No manual intervention is needed. Ensure the `data/` directory exists.

## Health Checks & Resource Limits

- Use `RUST_LOG=debug` or `RUST_LOG=info` to monitor the bot's health.
- Resource limits are strictly enforced via the `[resource_limits]` section in `config.toml` (`max_http_body_bytes`, and `max_cache_memory_bytes`/`cache_ttl_secs` for the HTTP response cache) to prevent memory bloating or OOM crashes on small servers.
- **Common Recovery**: If the bot panics at startup, use `--check-config` to verify the `config.toml`, and `--explain-config` to see what it enables in each guild. If API requests fail, verify API keys in `.env`.
- **Applying config changes**: `sudo systemctl reload brickbot` validates `config.toml` and restarts the bot in place; an invalid file is rejected and the bot keeps running (see README, "Reloading Configuration"). `SIGHUP` reloads rather than stops the bot, so a hand-started instance survives its terminal closing — stop it explicitly.

## Privacy & Data-Retention Policy

- The bot features an **Ambient Assistant** that triggers ephemeral interactions (only visible to the user) to protect privacy.
- Ambient training data is not logged by default. If enabled via `[ambient.training_data]`, data is retained based on `retention_days` and cleaned up automatically every 24 hours.
- Every ambient setting can be scoped to one guild with `ambient.*` keys in its `[[guilds]]` block: `ambient.channel_ids`, `ambient.set`, `ambient.part`, `ambient.debug_logging` and `ambient.training_data.*`. A guild's `ambient.training_data` replaces the global `[ambient] training_data` for that guild, so `ambient.training_data.enabled = false` opts a guild out of a global capture.
