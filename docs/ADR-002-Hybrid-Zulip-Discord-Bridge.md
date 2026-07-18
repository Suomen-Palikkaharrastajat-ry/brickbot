# ADR-002: Hybrid Bot Architecture: Bridging Zulip and Discord

## Status
Accepted

## Date
2026-07-03 (Updated: 2026-07-15)

## Context

Our community resides on Discord, where untrusted users can submit new content (e.g., map locations, events) or ask questions. However, managing submissions directly in Discord is often chaotic:
- It lacks advanced threading capabilities for long-running triage.
- It is difficult to separate trusted internal team discussions from public chatter.
- Granting trusted team members direct database access (e.g., PocketBase) creates friction and security concerns.

Conversely, our internal trusted team utilizes Zulip, which excels at topic-based threading and asynchronous communication. 

We need a reliable, low-friction mechanism to:
1. Vet and moderate community submissions from Discord before inserting them into our primary database (PocketBase).
2. Deliver asynchronous moderation updates (approvals, rejections, questions) back to Discord users securely.

---

## Decision

We will implement a **Hybrid Bot Architecture** within our existing Rust-based Discord bot to act as an event-driven middleware bridging Discord, Zulip, and PocketBase.

The bot will isolate the platforms while maintaining a lightweight tracking ledger in SQLite to map cross-platform interactions.

### Key Components

1. **Discord (Untrusted Intake)**: Utilizes native Discord Modals (via Slash Commands like `/events`) for structured data submission.
2. **Zulip (Trusted Triage)**: Receives mirrored data as dedicated Topics. Uses Zulip's Real-Time Events API (Long-Polling) to monitor text-commands (`@bot reply`) for bidirectional messaging and state changes.
3. **SQLite Ledger (State Mapping)**: Caches pending metadata and maps Zulip topics to Discord message IDs/Channels and PocketBase payloads.
4. **PocketBase (Source of Truth)**: Receives only fully vetted and approved data payloads from the bot.

---

## Architecture Blueprints

### Use Case A: Event / Map Input Moderation
*Designed for high-reliability vetting of community submissions before database insertion.*

```text
[Discord: User Form] ──> [Rust Bot] ──> [Zulip: Unique Topic Created]
                                                │
[PocketBase Event DB] <── [Rust Bot] <── [Zulip Admin `@bot reply approved`]
                                                │
[Discord DM to User]  <── [Rust Bot] <──────────┘
```

**The Flow:**
1. **Intake (Discord):** User triggers `/events` -> Fills out a native Discord Modal (structured data).
2. **Triage (Zulip):** Bot creates a dedicated Topic in an internal moderation stream, prints the data, and awaits review.
3. **Execution (PocketBase):** Admin replies in Zulip with `@bot reply approved`. The bot parses the command via long-polling, pushes the cached payload to PocketBase, and updates the Discord user.


## Asynchronous Zulip Updates & Delivery Fallback

Moderation decisions (approval, rejection, or questions) arrive asynchronously from Zulip. Because Discord ephemeral interaction tokens expire after 15 minutes, the bot cannot reply ephemerally to the original interaction.

**Delivery Mechanism:**
1. **Direct Message (Primary)**: The bot looks up the submitter's Discord user ID stored in the SQLite ledger and attempts to send a private DM. This DM contains the submission title, status (`approved`, `rejected`), and moderator rationale without leaking internal Zulip details.
2. **Fallback Mechanism**: If the user has DMs disabled, the bot records a `dm_failed` status. It then posts a *minimal, localized, public reply* in the original configured channel indicating a private update is available.
3. **Status Check**: The user can invoke the `/events status` interaction command to view the full details ephemerally, bypassing the DM restriction.

*Rule: Moderator reasoning and rationale are strictly delivered privately (DM or ephemeral status check), never in a public channel.*

---

## The State Tracking Ledger

To keep the external database (PocketBase) clean, the bot holds pending metadata locally in SQLite. 

```sql
CREATE TABLE inputs (
    id TEXT PRIMARY KEY,             -- Internal UUID
    source_user_id TEXT,             -- Discord User ID (for DM notifications)
    discord_channel_id TEXT,         -- Discord Channel/Thread ID
    zulip_stream TEXT,               -- E.g., "map-moderation"
    zulip_topic TEXT,                -- Unique topic name
    zulip_message_id INT,            -- The bot's initial prompt message ID
    payload_json TEXT,               -- Raw Discord form data waiting for approval
    status TEXT DEFAULT 'pending'    -- pending, approved, rejected
);
```

---

## Consequences

### Positive
- **Clean Separation of Concerns**: Untrusted users stay in Discord; trusted users stay in Zulip. PocketBase only receives clean, vetted data.
- **Improved Moderation UI**: Leveraging Zulip's topic threading means every submission gets its own dedicated discussion space without cluttering a single Discord channel.
- **Privacy & Security**: Long-polling removes the need for exposing an HTTP webhook endpoint to the internet. Moderation rationale remains private.

### Negative
- **State Management**: The bot is no longer completely stateless; it must maintain and manage the lifecycle of the `inputs` ledger.
- **Notification Persistence**: The bot must handle DM delivery failures and manage an inbox/status workflow so users can securely retrieve rejected event details.
