# Locations Command

## Overview
The `/locations` command lets community members propose map locations — exhibitions, stores,
flea markets and museums — and propose changes to existing ones. Submissions are not written
directly to the database: each one opens a moderation topic in Zulip, and only after a
moderator approves it is the record pushed to the PocketBase `locations` collection that backs
[Palikkakartta](https://kartta.palikkaharrastajat.fi).

It mirrors the [events command](events.md) and shares its Zulip moderation flow; see
[ADR-002](../ADR-002-Hybrid-Zulip-Discord-Bridge.md) for the design of that bridge.

## Trigger
- Triggered as `/locations` (or `/paikat` depending on the `locale` defined in `config.toml`).
- Takes one **optional** argument: `image` (`kuva`), an attachment used as the location's cover
  image.

## Features

1. **Propose a new location**:
   - A select menu chooses up to two location types (`exhibition`, `store`, `fleamarket`,
     `museum`, `other`), matching the collection's `maxSelect: 2` tags field.
   - "Propose New" opens a five-field modal: **Title** (required), **Address**,
     **Opening hours**, **URL** and **Description**.
   - The proposal is posted to the Zulip moderation stream under a `📍` topic.

2. **Propose changes to an existing location**:
   - A select menu lists up to 25 published locations as `Title | Address`.
   - "Propose Changes" opens the same modal, prefilled from the live record.
   - The proposal is posted under a `📌` topic and includes a unified diff against the current
     record.

3. **Coordinates are set by moderators**:
   - The Discord modal deliberately has no coordinate field. The YAML block in the Zulip topic
     always contains a `point` entry, with `lat` and `lon` set to `null` when unknown.
   - `point` is required by the collection, so approving a **new** location without coordinates
     is refused: the bot replies in the topic explaining how to set them, and the submission
     stays pending. Edits are unaffected — they keep the record's existing coordinates.

4. **Moderation commands** (in the Zulip topic, moderators only):
   - `approve` / `hyväksy` — publish as a draft.
   - `approve published` / `hyväksy julkaistuna` — publish immediately.
   - `reject` / `hylkää` — discard.
   - A ```` ```yaml ```` block containing only the fields to change — updates the stored payload
     and rewrites the topic message. This is how `point`, `start_date`, `end_date` and
     `image_description` get filled in.
   - Any other reply is forwarded to the submitter as a Discord DM.

5. **Localization**:
   - Everything the submitter sees follows the guild's `locale`: the command name and its
     `image` argument, the wizard prompts and buttons, the modal title and its field labels, and
     the validation errors. Keys live under `command.locations.*`, `modal.location.*` and
     `validation.*` in `locales/en-US.yml` and `locales/fi-FI.yml`.
   - Zulip moderation messages use the locale of the guild the submission came from.

6. **Result notification**:
   - On approval the topic is resolved with a link to
     `https://kartta.palikkaharrastajat.fi/#/locations/<id>` and the submitter is notified in
     Discord.

## Configuration

The command is disabled by default. Enable it globally or per guild in `config.toml`:

```toml
[pocketbase]
url = "https://data.palikkaharrastajat.fi"
collection = "events"
# Optional; defaults to "locations"
locations_collection = "locations"

[commands.locations]
enabled = true
enable_edit = true             # show the "Propose Changes" flow
enable_propose = true          # show the "Propose New" flow
enable_fallback_mention = false # @-mention the submitter if a DM cannot be delivered

[[guilds]]
guild_id = 123456789
[guilds.commands.locations]
enabled = false # Disables /locations for this specific guild
enable_fallback_mention = false
```

A guild's `[guilds.commands.locations]` section replaces the global one as a whole: a flag it leaves out takes its default, not the global value. The defaults are `enabled`, `enable_edit` and `enable_propose` **off** and `enable_fallback_mention` **on**, so repeat `enable_fallback_mention = false` in every guild section if public mentions should stay off.

A `[zulip]` section is required for the moderation flow; without it submissions are dropped.

## Implementation

| Concern | Location |
| --- | --- |
| Slash command definition | `src/commands/locations.rs` |
| Wizard, modals, validation | `src/workflows/locations/` |
| PocketBase reads | `src/workflows/locations/pocketbase.rs` |
| PocketBase writes | `src/pocketbase.rs` (`push_record_data`) |
| Zulip moderation routing | `src/zulip/moderation.rs` (`ModerationKind`) |
