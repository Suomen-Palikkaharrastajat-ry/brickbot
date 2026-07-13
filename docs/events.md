# Events Architecture & Sync Flow

This document details the architectural implementation of the event synchronization system in Brickbot, particularly how it integrates with PocketBase and Discord Scheduled Events.

## Overview

The events system bridges community events managed via Discord (using the `/events` wizard) and a backend PocketBase CMS. This allows for a structured event management process where events can be curated, moderated, and automatically synced to Discord.

## Bi-Directional Event Flow

The flow of event data operates in a pseudo-bi-directional loop between Discord and PocketBase:

1. **Submission (Discord -> Zulip -> PocketBase)**
   - Users submit or edit events via the Discord `/events` command modals.
   - The bot creates a draft submission in the local SQLite database (`inputs` table) to prevent data loss in case of delivery failure.
   - The payload is dispatched to a Zulip moderation stream for review.
   - Moderators review and then manually enter/publish the event in PocketBase.

2. **Synchronization (PocketBase -> Discord)**
   - Once an event is published in PocketBase (state='published'), the bot automatically pulls the event data.
   - The bot pushes the data to Discord by creating or updating an "External" Discord Scheduled Event.
   - The mapping between PocketBase's Record ID (`uid`) and Discord's Scheduled Event ID is maintained statelessly by embedding markers (`🆔`, `🕒`) in the Discord event description.

## PocketBase Integration

The integration with PocketBase (`src/events_sync`) operates via two main mechanisms:

- **Initial Full Sync:** On startup (or triggered manually via `/events sync`), the bot fetches all `published` events via the PocketBase REST API. It compares the fetched events against the live Discord Scheduled Events (using the description markers), creating, updating, or deleting as necessary to match PocketBase.
- **Realtime Updates (SSE):** The bot maintains a Server-Sent Events (SSE) connection to PocketBase's `/api/realtime` endpoint. When a record in the events collection is created, updated, or deleted, PocketBase broadcasts a message. The bot intercepts these messages and immediately applies the change to the corresponding Discord Scheduled Event.

## Cover Image Mapping

Discord Scheduled Events support cover images. When an event in PocketBase includes an image, the bot seamlessly maps it:
- The PocketBase image file is retrieved using the constructed URL: `{pb_url}/api/files/{collection}/{record_id}/{filename}`.
- The image bytes are downloaded via the bot's internal `HttpClient` (which includes caching via `moka`).
- The bytes are attached to the Discord API request (`CreateAttachment::bytes`) when creating or editing the Scheduled Event.

## Draft Storage and Resilience

To ensure a resilient user experience during the `/events` wizard flow, the bot utilizes local draft storage (`src/db.rs`):
- When a user submits an event, the form data is immediately saved to the `inputs` table with a `draft` status.
- If the downstream routing (e.g., to Zulip) fails, or if validation fails, the data is preserved.
- Users can use the "Edit & Retry" interactive workflow to recover their draft, modify it, and resubmit without needing to re-type the entire event description.
