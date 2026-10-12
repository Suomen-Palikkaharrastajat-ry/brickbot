# Brickbot - An opinionated Rust Discord bot

> **Note for Developers & AI Agents:** Please refer to [`AGENTS.md`](./AGENTS.md) for architectural details, build configuration, and development context.

A statically linked Rust-based Discord bot that polls RSS/ATOM feeds, OPML files, and synchronizes community events from PocketBase. It provides an ambient conversational assistant that monitors chat and contextually offers information about LEGO sets, parts, and upcoming events.

## Setup & Usage with Discord

### 1. Create a Discord Bot
1. Go to the [Discord Developer Portal](https://discord.com/developers/applications).
2. Click **New Application** and give your bot a name.
3. Navigate to the **Bot** tab on the left sidebar.
4. Click **Reset Token** to generate a new bot token. **Copy this token**, as you will need it later.
5. Under **Privileged Gateway Intents**, enable **Message Content Intent** (and Server Members Intent if you plan to expand the bot's features).
6. Save Changes.

### 2. Invite the Bot to Your Server
1. In the Developer Portal, go to the **OAuth2 > URL Generator** tab.
2. Under **Scopes**, select `bot` and `applications.commands`.
3. Under **Bot Permissions**, select the following permissions required by the bot's features:
   - **Create Events** & **Manage Events**: Required for synchronizing events from PocketBase to Discord Server Scheduled Events.
   - **View Channels**: Required for the ambient assistant to monitor chat.
   - **Send Messages**: Required to send ambient suggestions and command responses.
   - **Embed Links**: Required to display rich data for LEGO sets and parts.
   - **Use Slash Commands**: Required for the `/events` wizard command.
   - **Read Message History**: Recommended for reliably replying to existing messages.
4. Copy the generated URL at the bottom and paste it into your browser to invite the bot to your desired server.

### 3. Get Your Target Channel & Guild IDs
1. Open the Discord app.
2. Go to **User Settings > Advanced** and turn on **Developer Mode**.
3. Right-click the server name and select **Copy Server ID** (Guild ID).
4. Right-click the channel where you want the bot to post and select **Copy Channel ID**.

### 4. Configure the Environment & Bot Settings

Copy the example environment file:
```bash
cp .env.example .env
```
Edit the `.env` file to include your core credentials:
```env
DISCORD_TOKEN=your_token_here
DATABASE_URL=sqlite:data/bot.db
ZULIP_API_KEY=your_zulip_api_key_here
POCKETBASE_IMPERSONATE_AUTH_TOKEN=your_pb_token_here
REBRICKABLE_API_KEY=your_rebrickable_api_key_here
BRICKSET_API_KEY=your_brickset_api_key_here
```

`REBRICKABLE_API_KEY` (free, from <https://rebrickable.com/api/>) is what `/part`, the
`/build` part picker and part autocomplete look parts up with. Without it every part lookup
fails, and autocomplete falls back to the built-in catalogue of everyday sizes. `BRICKSET_API_KEY`
is only used by `/set`.

Part lookups are remembered in a separate SQLite database (`data/cache.db` by default), so
suggestions get better the more the bot is used and survive a restart. It holds nothing that
cannot be re-fetched: deleting the file is a clean rebuild. See `[parts_cache]` in
`config.toml.example`.

Create and configure your `config.toml` (or edit the existing one):
```toml
# Global bot name (optional)
bot_name = "Palikkaharrastaja"
# Global default locale (optional; guild config wins, and "fi-FI" applies when neither is set)
locale = "fi-FI"

ambient_debug_logging = true

poll_interval = 3600
cache_ttl_secs = 600

# Zulip Integration (optional)
[zulip]
url = "https://forum.palikkaharrastajat.fi"
bot_email = "palikkaharrastajat-bot@forum.palikkaharrastajat.fi"
moderation_stream = "Bottilaatikko"
moderators = ["moderator@example.com"]

# PocketBase Integration (optional)
[pocketbase]
url = "https://data.palikkaharrastajat.fi"
collection = "events"

# Interaction toggles (default is true)
[interactions]
set = true
part = true

# Command toggles
[commands.events]
enabled = true
enable_edit = true
enable_propose = true
enable_fallback_mention = false

# Same switches as [commands.events]; off by default
[commands.locations]
enabled = true
enable_edit = true
enable_propose = true
enable_fallback_mention = false

[[guilds]]
guild_id = 123456789012345678 # Your Guild ID here
locale = "fi-FI"

# Any [commands.*] section can be overridden for one guild (optional).
# A guild section replaces the global one whole, so flags left out take
# their defaults; enable_fallback_mention defaults to true.
[guilds.commands.locations]
enabled = false
enable_fallback_mention = false

# Publish new feed articles to a channel of this guild (optional, repeatable).
# Without feed_urls/opml_urls the section follows every global [[feeds]] entry.
[[guilds.feeds]]
channel_id = 234567890123456789
feed_urls = ["https://example.com/feed1.rss"]

[[feeds]]
    # List of Feed URLs
    feed_urls = [
        "https://example.com/feed1.rss",
        "https://example.com/feed2.rss"
    ]
    # List of OPML URLs
    opml_urls = ["https://example.com/feeds.opml"]
```
> [!IMPORTANT]
> The `guild_id` must be a valid, non-zero numeric ID.

### 5. Run the Bot
To run the bot locally using the Nix devShell:
```bash
devenv shell cargo run
```

#### Debugging
The bot uses the `tracing` library. To enable verbose debug logging (useful for troubleshooting webhooks or API errors), start the bot with the `RUST_LOG` environment variable set to `debug`:
```bash
RUST_LOG=debug devenv shell -- cargo run
```

To build the production binary:
```bash
nix build
```
The resulting executable will be located at `result/bin/brickbot`.

#### Systemd Service

If you have a statically built binary (e.g. from `nix build`), you can run it as a `systemd` service. Create a file `/etc/systemd/system/brickbot.service`:

```ini
[Unit]
Description=Brickbot Discord Bot
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=brickbot
Group=brickbot
# Ensure the working directory contains your data/ folder and config files
WorkingDirectory=/opt/brickbot
ExecStart=/opt/brickbot/brickbot
# `systemctl reload brickbot`: check the new config, then ask the bot to apply it
ExecReload=/opt/brickbot/brickbot --check-config
# On NixOS there is no /bin/kill: use ${pkgs.coreutils}/bin/kill
ExecReload=/bin/kill -HUP $MAINPID
Restart=always
RestartSec=5
Environment="RUST_LOG=info"

[Install]
WantedBy=multi-user.target
```

Enable and start the service:
```bash
sudo systemctl daemon-reload
sudo systemctl enable --now brickbot
```

#### Reloading Configuration

After editing `config.toml`, apply it without stopping the service:

```bash
sudo systemctl reload brickbot
journalctl -u brickbot -f   # watch the outcome
```

What happens:

1. `brickbot --check-config` validates the file. If it is invalid, `systemctl reload` fails, the validation errors are in the journal, and the running bot is not touched.
2. The bot receives `SIGHUP`, validates the file once more itself (so a plain `kill -HUP <pid>` is just as safe), and on success restarts in place: it closes the Discord connection and the databases and re-executes itself with the same command line. The PID does not change, so systemd sees one uninterrupted service.

`systemctl reload` returns as soon as the signal is sent, not when the bot is back: follow the journal to see the restart complete.

Expect the bot to be offline for a few seconds while it reconnects to Discord. As with a restart, build threads, drafts and wizard sessions are stored in SQLite and survive, while a render still waiting in its debounce window may be dropped and Zulip messages arriving during the gap can be missed. A reload sent while the bot is still starting up is queued and applied once startup has finished.

If `ExecStart` passes `--config <path>`, repeat it on the `--check-config` line. To validate by hand, run `/opt/brickbot/brickbot --check-config` from the working directory.

A reload is **not** enough for:

- **Environment variables and secrets** (`DISCORD_TOKEN`, `DATABASE_URL`, `REBRICKABLE_API_KEY`, `.env`, `Environment=` lines): the restarted bot inherits the old environment. Use `sudo systemctl restart brickbot`.
- **Changes to the unit file itself**: `sudo systemctl daemon-reload && sudo systemctl restart brickbot`.

Because the bot re-executes the path it was started with, a reload also picks up a binary that was replaced at that path.

> [!NOTE]
> `SIGHUP` now means "reload", not "quit". A bot started by hand in a terminal therefore keeps running when that terminal or SSH session closes — stop it with `Ctrl+C` or `kill <pid>` before starting another, or two instances will answer.

## Features & Commands

### RSS, ATOM & OPML Polling
The bot automatically fetches the defined `feed_urls` and `opml_urls` at the specified `poll_interval`. Feeds are parsed to support both RSS and ATOM formats.

A guild can have new articles posted to its channels with `[[guilds.feeds]]` sections. Each source is announced separately: up to three new articles of a source get one message each, more are listed in a single message for that source. Only articles that appear after the bot was last (re)started are announced. See [`docs/news.md`](docs/news.md).

### Event Synchronization
The bot continuously syncs events from PocketBase into Discord's native Scheduled Events system. 
- **`/events` Command:** A multi-step wizard to list upcoming events, submit new ones (sent to Zulip for moderation), or trigger a manual sync.

### Map Locations
- **`/locations` Command:** A wizard to propose new map locations (exhibitions, stores, flea markets, museums) or changes to existing ones. Proposals go to Zulip for moderation and, once approved, are published to the PocketBase `locations` collection behind [Palikkakartta](https://kartta.palikkaharrastajat.fi). Coordinates are set by moderators in the Zulip topic. See [docs/commands/locations.md](docs/commands/locations.md).


### Collaborative Building
- **`/build` Command:** Opens a thread and turns it into a shared LEGO workspace. Members describe the model in plain sentences — `Add a red 2x4 brick at origin.`, `Build a wall 10 studs long 6 bricks high.`, `Undo.` — and the bot re-renders it after every accepted command and can export it as a BrickLink Studio `.io` file. Ordinary conversation in the thread is ignored, so the thread stays usable for talking. See [`docs/commands/build.md`](docs/commands/build.md).
- Real part shapes require the LDraw library: run `make fetch-ldraw` once. Until then the bot draws simple boxes.

### Ambient Assistant
The bot monitors chat passively using an Ambient Assistant:
- **LEGO Sets & Parts:** If the bot detects discussion about LEGO sets or parts, it offers a button to fetch details and images from Rebrickable/Brickset.

### Interaction Privacy Policy
- **Ephemeral Interaction Responses:** All user-initiated command flows (like `/events` or interacting with ambient components) begin with an ephemeral response visible only to the invoking user.
- **Modals:** Form inputs (modals) are presented directly to the user and generate ephemeral submit responses.
- **Public Discovery:** Ambient detection posts the smallest acceptable public component message (e.g., a single button) with no personal state. Upon interaction, the flow becomes ephemeral.

## Zulip Integration Configuration

The bot supports an advanced Hybrid Architecture that bridges Discord (for untrusted community input) and Zulip (for trusted internal triage and support). 

### 1. Create a Zulip Bot
1. Log in to your self-hosted Zulip instance as an administrator.
2. Navigate to **Settings (gear icon) > Personal settings > Bots**.
3. Click **Add a new bot**.
4. Choose **Generic bot** for the Bot type.
5. Give it a name (e.g., "Brickbot") and click **Create bot**.
6. Note down the **Bot email** and the **API key**.

### 2. Configure config.toml
Add the `[zulip]` section to your `config.toml` using the credentials from the previous step:

```toml
[zulip]
url = "https://your-zulip-instance.example.com"
bot_email = "brickbot-bot@your-zulip-instance.example.com"
moderation_stream = "moderation"
moderators = ["moderator@example.com"]
```

### 3. Automatic Event Polling
The bot uses Zulip's Real-time Events API (Long-Polling). This means it automatically connects outbound to your Zulip instance and holds the connection open, waiting for new events (like `@bot reply <answer>`).
**No incoming webhook configuration, open ports, or public IPs are required!** As long as the bot has internet access to your Zulip URL, it will instantly receive replies and approvals.

[Invite Brickbot to your server](https://discord.com/oauth2/authorize?client_id=1519031617132302597&permissions=17602923547648&integration_type=0&scope=bot+applications.commands)