# Ambient Conversational Assistant

This document outlines the current implementation of the Ambient Conversational Assistant in Brickbot, based on the architectural decisions defined in [ADR-001](../ADR-001-Ambient-Conversational-Discord-Assistant.md).

## Overview

The ambient assistant passively observes messages in Discord channels to detect specific topics (such as LEGO sets or parts). When it detects a topic with sufficient confidence, it presents lightweight, interactive suggestions using Discord Buttons. It avoids interrupting conversations, respects user preferences, and employs cooldown mechanisms to prevent channel noise.

## Topic Detection (`src/ambient.rs`)

## Interaction Privacy Policy and Vocabulary
- **Ephemeral interaction response:** A response carrying Discord's `EPHEMERAL` flag, visible only to the invoking user. All user-initiated command flows must begin with this.
- **Application command:** e.g., `/events`. Its initial response should be an ephemeral wizard message containing buttons or string select menus.
- **Message component interaction:** Selecting an event action or clicking a button.
- **Modal interaction response:** A client popup form, leading to a **modal submit interaction**.
- **Component message:** The public discovery message posted by ambient detection. It contains minimal public text and a component (like a button) that initiates an ephemeral flow.

Topic detection happens locally without calling external LLM APIs for every message, ensuring speed and respecting privacy.

### Detection Mechanisms

- **Keyword Matching:** Uses `AhoCorasick` for fast, multi-pattern keyword detection. Keywords are weighted and grouped by topic (e.g., "lego set", "osa"). It supports both English and Finnish keywords.
- **Regex Patterns:** Identifies 5-7 digit numbers (common for LEGO set numbers) to boost confidence scores.

### Supported Topics

The current implementation can detect the following topics:
- `LegoSet`
- `LegoPart`

### Confidence Scoring

Matches accumulate scores to determine the `Confidence` level:
- **Low (Score < 2):** Ignored.
- **Medium (Score 2-3):** Minimum threshold required to trigger a suggestion.
- **High (Score >= 4):** Strong detection (e.g., multiple keyword hits, or keyword + exact set number).

## Execution Flow (`src/main.rs`)

The `EventHandler` in `src/main.rs` processes incoming messages:

1. **Passive Observation:** Every non-bot message is passed through `detect_topic`.
2. **User Consent Check:** Queries the database (`ambient_user_preferences`) to verify if the user has opted out (`ignore_all`). If so, the bot aborts.
3. **Threshold Check:** Only proceeds if the confidence is `Medium` or `High`.
4. **Cooldown Enforcement:**
   - **Item Cooldown:** If a specific item (e.g., a set number) is detected, it checks `ambient_item_cooldowns`. The exact same item won't be suggested again in the same channel for **12 hours**.
   - **Topic Cooldown:** Checks `ambient_cooldowns` to ensure the same general topic isn't suggested more than once every **30 minutes** per channel.
5. **Suggestion Delivery:** If cooldowns pass, the bot records the new cooldown timestamps in the database and dispatches a localized suggestion message using Discord buttons.

## Workflows and Localization (`src/workflows/ambient.rs`)

When the bot decides to interact, it delegates the message creation to `workflows/ambient.rs`:

- **`generate_suggestion_text`:** Creates a localized prompt based on the detected topic and the guild's configured locale.
- **`generate_suggestion_components`:** Constructs Discord Message Components (Buttons). These buttons provide explicit user intent (e.g., "Show details", "Ignore") and avoid the pitfalls of emoji reactions mentioned in the ADR.

## Database & State Management (`src/db.rs`)

The SQLite database backs the ambient state, tracking:
- `ambient_logs`: Records detections for analytics and training.
- `ambient_cooldowns`: Stores the last suggested time for topics per channel.
- `ambient_item_cooldowns`: Stores the last suggested time for specific items (like set IDs) per channel.
- `ambient_user_preferences`: Tracks opt-out preferences (`ignore_all`).

This implementation guarantees that the ambient assistant remains helpful and unobtrusive in active communities.
