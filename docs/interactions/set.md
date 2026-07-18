# Set Interaction

## Overview
The `set` interaction provides detailed information about LEGO sets. Unlike traditional slash commands, this interaction is primarily triggered contextually via the ambient assistant when it detects discussions about LEGO sets (`Topic::LegoSet`).

## Trigger
- Triggered via the `workflow_set_search` button presented by the ambient assistant.
- Users input the set number or keyword in a modal dialog (`modal_set_search`).

## Features
- **Set Data**: Fetches data (name, year, theme, pieces, rating, image) using the `fetch_set` function (connecting to external APIs like Brickset).
- **Related Articles**: Searches the local `feed_items` database for any recent RSS/Atom articles matching the set number and appends links to the results directly beneath the set embed.
- **Service Links**: Users can select which third-party services they want links for via a dropdown menu (`update_services_set`). Supported services include:
  - BrickLink
  - LEGO.com
  - Brickset
  - Rebrickable
- **Localization**: Fully localized using `rust-i18n` (supports `en-US` and `fi-FI`).

## Internal Implementation
- Handled primarily in `src/commands/set.rs`, `src/workflows/ambient.rs`, and `src/workflows/search.rs`.
- `build_set_message` is a pure function that constructs the Discord embed, ensuring testability without network or Discord API dependencies.
