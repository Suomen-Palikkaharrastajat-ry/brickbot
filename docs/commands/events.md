# Events Wizard & Synchronization

## Overview
The `/events` command is a multi-step wizard designed to manage, synchronize, and interact with community events. It acts as the central hub for event-related actions, completely replacing the legacy standalone commands for asking, syncing, and submitting.

The bot natively synchronizes events from the configured PocketBase REST API directly into Discord's native Server Scheduled Events.

> [!IMPORTANT]
> **Required Permissions**: The bot requires the **Create Events** and **Manage Events** permissions in your Discord server to automatically create, update, and delete the synchronized Scheduled Events.

## Trigger
- Triggered per-guild as either `/events` or `/tapahtumat` depending on the `locale` defined in `config.toml` (falls back to `default_locale` if unspecified).
- The command supports two **optional** arguments to bypass the initial wizard and provide advanced functionality:
  - `action`: A dropdown to directly select an action (`List Events`, `Sync Events`, `Submit Event`, `Edit Event`).
  - `image`: An optional image attachment, used when proposing a new event.
- If no arguments are provided, the command responds with an ephemeral prompt offering the wizard options.
- The event listing interface can also be triggered via the `workflow_events_list` button when the ambient assistant detects discussions about events (`Topic::Events`).

## Wizard Options

1. **List Events**: 
   - Fetches and lists the top 5 upcoming events in Discord (both synchronized from PocketBase and manually created).


2. **Sync Events**:
   - Manually triggers an immediate synchronization of the events from PocketBase. 
   - The bot pulls `published` events, creates Discord Scheduled Events (marked as `External` events), and keeps their titles, start times, locations (favoring hyperlinks), descriptions, and cover images in sync. It automatically deletes the scheduled events if they are removed from PocketBase.

3. **Submit Event**:
   - First presents a Type dropdown (Exhibition, Event, or Competition), with Event selected by default.
   - Opens a five-field modal for Title, Dates, Location, URL, and Description. The selected Type is carried in the modal interaction rather than entered as text.
   - **Moderation Workflow**: Submissions are saved to the bot's local database and routed to a dedicated Zulip moderation stream as proposals. They must be reviewed, edited (if necessary), and approved via Zulip bot commands before they are sent to PocketBase.

4. **Edit Event**:
   - Presents a dropdown to select an existing synchronized event.
   - Then presents a Type dropdown with the event's current type pre-selected, before opening a modal pre-filled with Title, Dates, Location, URL, and Description.
   - **Moderation Workflow**: Edits follow the exact same proposal flow as new submissions, routing to Zulip for approval before altering PocketBase data.

## Zulip Moderation Commands
When an event or edit is proposed, it appears in Zulip with its full JSON payload. Moderators can use the following commands by replying to the specific topic:
- `approve` or `✅`: Approves the event and pushes it to PocketBase as a `draft` (requires manual publishing in PocketBase UI).
- `approve published`: Approves the event and pushes it to PocketBase as `published`.
- `reject` or `❌`: Rejects the event. The user is notified via Direct Message on Discord.
- `edit { "title": "new title", ... }`: Replaces the current proposed JSON payload with the newly provided JSON. The bot will acknowledge the update, and the moderator can then `approve` it.
- **Any other text**: Treated as a direct reply. The bot will forward the message back to the original Discord user via Direct Message, allowing for two-way communication between the user and moderation team.

> [!NOTE]
> **Moderation Notifications**: The bot attempts to notify users of moderation actions via private Direct Messages. If the user has disabled server DMs, the bot will fall back to sending a public ping in the Discord channel where the interaction originally occurred (omitting details for privacy, prompting them to use `/events status`). This public fallback behavior can be disabled in `config.toml` by setting `enable_fallback_mention = false` under `[commands.events]`.

## Event Cover Images
Due to Discord's Modal interaction framework lacking support for file attachments, the bot uses a hybrid approach to support event cover images:
1. The user must provide the image via the optional `image` slash command argument while simultaneously setting the `action` argument to `Submit Event`.
2. The bot temporarily stores the image URL in a `drafting` state in the SQLite database and binds a unique UUID to the modal's `custom_id` after the Type selection.
3. When the text modal is submitted, the bot retrieves the stored image URL and bundles it into the final Zulip proposal payload.

## Internal Implementation
- Handled in `src/commands/events.rs`, `src/events_sync.rs`, and `src/workflows.rs`.
- Background syncing is handled in `src/events_sync.rs` using the `serenity` `ScheduledEvent` API.
- The bot maps PocketBase UIDs to Discord Event IDs statelessly using marker tokens embedded in the Discord event description, ensuring it only modifies its own synced events. Manually created Discord events are fully respected.
- Submissions and edits are inserted into the `inputs` table and forwarded to Zulip using `src/zulip`.
