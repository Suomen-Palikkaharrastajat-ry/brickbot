# Build Interaction

## Overview
Unlike the `set` and `part` interactions, the `build` interaction is not triggered by the ambient assistant. It is scoped to a thread opened by [`/build`](../commands/build.md), and inside that thread the bot reads **every** message and decides for itself whether it was meant as a building instruction.

That makes its silence policy the most important thing about it. A build thread is a normal conversation as well as a workspace, so the bot must ignore the conversation completely and act only on the grammar it recognises.

## Trigger
- The session is created by the `/build` slash command, which opens a public thread and records a row in `build_sessions` keyed by the thread's channel id.
- Afterwards there is **no** trigger word, prefix or mention: every message in the thread is parsed.
- Render messages carry three buttons, whose `custom_id`s each end in the 12-character session id: `build_export:<id>`, `build_undo:<id>` and `build_finish:<id>`.
- When a part cannot be resolved, a `build_part_pick` select menu is posted, reusing the `/part` search results.

## Features

- **Silence Policy**: A message is only a command when its first word is a verb the language knows, or a question word. Everything else returns `Ignored` before the parser is even entered, producing no reply, no reaction and no render. A sentence that opens with a known verb but fails to parse *is* reported, with the word it stopped on.
- **Feedback By Reaction**: Accepted commands get ✅, comments (`Note:` / `TODO:`) get 📝, and problems get ❓ plus one short message. This keeps the thread readable when several people are building at once.
- **Collaborative State**: The session is looked up by channel, never by owner, so every member of the thread edits the same model. This is why `build_sessions` exists rather than reusing `workflow_sessions`, whose accessors are all owner-scoped.
- **Debounced Rendering**: A burst of commands coalesces into one render, so four people typing simultaneously produce one picture rather than four.
- **Part Disambiguation**: A part the built-in catalogue does not cover is handed to `part_interaction`, the same function behind [`/part`](part.md). Its `SearchResults` variant supplies the dropdown; the pending sentence is retried once a part is chosen.
- **Localization**: All output is localized with `rust-i18n`. The *input* grammar is deliberately not translated — it is a language, not UI text — so verbs, colours and units stay English in every locale. A Finnish keyword table can be added later without touching the parser.

## Internal Implementation
- `src/build/dispatch.rs` — `process_message`, the pure entry point. It returns the operations to record, the replies, the reactions and the render/export/finish flags. `Outcome::is_silent()` is the empty case, and is asserted against a table of real chat lines.
- `src/build/report.rs` — every user-facing string, in both locale files.
- `src/build/session.rs` — the Discord shell: thread creation, persistence, the render loop and the buttons. It contains no decisions of its own.
- `src/commands/build.rs` — the command declaration and the pure message builders.
- `src/db/build.rs` — session and revision storage, looked up by channel id.
- `src/main.rs` — the session lookup is hoisted **above** the `ambient_channel_ids` gate in the `message` handler. A thread has its own channel id, so it is never in that list; without the hoist every build command would be silently dropped. Returning early also keeps build traffic out of the ambient assistant's two-hour per-user cooldown.
- `src/workflows/mod.rs` — routes the three `build_*` button ids to `handle_build_button`.
