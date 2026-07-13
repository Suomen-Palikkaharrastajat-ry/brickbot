# Operator Runbook

## Required Discord Intents & Permissions

When inviting the bot or configuring the Discord Developer Portal, the following intents and scopes are strictly required:

### Intents (Privileged)
- **Message Content Intent**: Required to process commands and read chat for the ambient assistant.
- **Guilds / Guild Messages**: Required for basic bot operation and reading channel events.

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

## Database Backup, Restore, and Migration

The bot uses SQLite (`data/bot.db`).

- **Backup**: Copy `data/bot.db` and any `-wal`/`-shm` files to a safe location while the bot is stopped, or use the `.backup` command in `sqlite3` CLI.
- **Restore**: Stop the bot and replace the `data/bot.db` file.
- **Migration**: Handled automatically on startup via `sqlx::migrate!()`. No manual intervention is needed. Ensure the `data/` directory exists.

## Health Checks & Resource Limits

- Use `RUST_LOG=debug` or `RUST_LOG=info` to monitor the bot's health.
- Resource limits are strictly enforced via the `[resource_limits]` section in `config.toml` (e.g., `max_http_body_bytes`, `max_cache_memory_bytes`) to prevent memory bloating or OOM crashes on small servers.
- **Common Recovery**: If the bot panics at startup, use `--check-config` to verify the `config.toml`. If API requests fail, verify API keys in `.env`.

## Privacy & Data-Retention Policy

- The bot features an **Ambient Assistant** that triggers ephemeral interactions (only visible to the user) to protect privacy.
- Ambient training data is not logged by default. If enabled via `[ambient_training_data]`, data is retained based on `retention_days` and cleaned up automatically every 24 hours.
