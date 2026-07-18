# Set Command

## Overview
The `/set` command provides an easy way to search and display information about a specific LEGO set directly within Discord. It automatically queries the Brickset API for set details, cross-references internal databases for related news or articles, and provides quick external links to various LEGO platforms.

## Trigger
- Triggered as `/set` (or `/setti` depending on the `locale` defined in `config.toml`).
- The command takes one **required** argument: `query` (a set number e.g. `42083` or a search term e.g. `Bugatti`).

## Features

1. **Set Information Display**:
   - Displays the set's title, year of release, theme, subtheme, piece count, and rating.
   - Embeds the official set cover image.

2. **Search Capability**:
   - If the `query` matches multiple sets, the bot will return an ephemeral dropdown menu allowing the user to select the specific set they intended.

3. **External Link Integration**:
   - The bot dynamically generates and displays links to:
     - BrickLink
     - Brickset
     - LEGO.com
     - Rebrickable

3. **Related Articles**:
   - If configured with an RSS feed, the bot automatically searches its internal database for articles matching the set number, name, or theme, and links them below the set information.

4. **Service Customization**:
   - Upon running the command, the user is presented with a dropdown menu allowing them to toggle which external services (BrickLink, LEGO.com, etc.) they want visible.
   - The bot persists these preferences per-user in the local SQLite database, ensuring that future queries default to the user's preferred platforms.

## Configuration
The command can be enabled or disabled globally or on a per-guild basis using the `config.toml` file:

```toml
[commands.set]
enabled = true

[[guilds]]
guild_id = 123456789
[guilds.commands.set]
enabled = false # Disables /set for this specific guild
```
