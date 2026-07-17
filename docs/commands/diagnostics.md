# Diagnostics Command

## Overview
The `/diagnostics` command acts as a quick health-check utility for the bot administrators. It probes the various internal and external subsystems the bot depends on and reports their statuses in an ephemeral Discord message. 

## Trigger
- Triggered as `/diagnostics` (or `/diagnostiikka` depending on the `locale` defined in `config.toml`).
- The command takes no arguments.
- It returns an ephemeral embed visible only to the user who invoked it.

## Features
The command verifies the connectivity and basic operational status of the following components:

1. **Database (`data/bot.db`)**: 
   - Executes a lightweight `SELECT 1` query via SQLx to ensure the SQLite database is healthy, unlocked, and responsive.
   
2. **PocketBase Integration**:
   - If `[pocketbase]` is configured, it attempts a read request against the REST API (fetching events) to confirm external network connectivity and correct routing.
   - If unconfigured, reports as `Disabled`.

3. **Zulip Integration**:
   - If `[zulip]` is configured, it performs a lightweight GET request to the Zulip API using the configured bot email and API key (via `ZULIP_API_KEY`) to test the authentication and connection.
   - If unconfigured, reports as `Disabled`.

## Configuration
The `/diagnostics` command is **disabled by default** to keep the command list clean. It can be enabled globally or per-guild using `config.toml`:

```toml
[commands.diagnostics]
enabled = true

[[guilds]]
guild_id = 123456789
[guilds.commands.diagnostics]
enabled = true # Or false to disable just for this guild
```
