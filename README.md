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
```

Create and configure your `config.toml` (or edit the existing one):
```toml
# Global bot name (optional)
bot_name = "Palikkaharrastaja"

ambient_logging = true

poll_interval = 3600
cache_ttl_secs = 600

# Zulip Integration (optional)
[zulip]
url = "https://forum.palikkaharrastajat.fi"
bot_email = "palikkaharrastajat-bot@forum.palikkaharrastajat.fi"
moderation_stream = "Bottilaatikko"
support_stream = "Bottilaatikko"

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

[[guilds]]
guild_id = 123456789012345678 # Your Guild ID here
locale = "fi-FI"

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

## Features & Commands

### RSS, ATOM & OPML Polling
The bot automatically fetches the defined `feed_urls` and `opml_urls` at the specified `poll_interval`. Feeds are parsed to support both RSS and ATOM formats.

### Event Synchronization
The bot continuously syncs events from PocketBase into Discord's native Scheduled Events system. 
- **`/events` Command:** A multi-step wizard to list upcoming events, submit new ones (sent to Zulip for moderation), or trigger a manual sync.


### Ambient Assistant (LEGO & Support)
The bot monitors chat passively using an Ambient Assistant:
- **LEGO Sets & Parts:** If the bot detects discussion about LEGO sets or parts, it offers a button to fetch details and images from Rebrickable/Brickset.
- **Support Proxy:** Messages sent in channels with "help" in their name are automatically mirrored to the Zulip support stream, enabling the team to assist Discord users without leaving Zulip.

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
support_stream = "support"
```

### 3. Automatic Event Polling
The bot uses Zulip's Real-time Events API (Long-Polling). This means it automatically connects outbound to your Zulip instance and holds the connection open, waiting for new events (like `@bot reply <answer>`).
**No incoming webhook configuration, open ports, or public IPs are required!** As long as the bot has internet access to your Zulip URL, it will instantly receive replies and approvals.

[Invite Brickbot to your server](https://discord.com/oauth2/authorize?client_id=1519031617132302597&permissions=8806830525440&integration_type=0&scope=bot+applications.commands)