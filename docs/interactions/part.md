# Part Interaction

## Overview
The `part` interaction provides detailed information about LEGO parts. It is triggered contextually via the ambient assistant when it detects discussions about LEGO parts (`Topic::LegoPart`).

## Trigger
- Triggered via the `workflow_part_search` button presented by the ambient assistant.
- Users input the part number or keyword in a modal dialog (`modal_part_search`).

## Features
- **Part Data**: Fetches data (name, production years, mold variants, alternates, print details) using the `fetch_part` function (connecting to the Rebrickable API).
- **Service Links**: Users can select which third-party services they want links for via a dropdown menu (`update_services_part`). Supported services include:
  - BrickLink
  - Rebrickable
- **Localization**: Fully localized using `rust-i18n`.

## Internal Implementation
- Handled primarily in `src/commands/part.rs`, `src/workflows/ambient.rs`, and `src/workflows/search.rs`.
- `build_part_message` constructs the Discord embed cleanly, keeping external fetching separated from message formatting.
