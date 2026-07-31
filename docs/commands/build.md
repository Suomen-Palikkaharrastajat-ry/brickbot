# Build Command

## Overview
The `/build` command starts a collaborative LEGO building session in a Discord thread. It is the only slash command in the feature: once the thread exists, the bot listens to the conversation in it and interprets plain sentences as building instructions, re-rendering the model after every accepted command. The result can be exported as a BrickLink Studio file.

The language it understands is a controlled natural language — English-like, quick to type, and deterministic. It is parsed locally by a hand-written recursive-descent parser; no external language model is involved, which keeps interpretation reproducible and matches the privacy stance in [the interactions overview](../interactions/README.md).

## Trigger
- Triggered as `/build` (or `/rakenna` depending on the `locale` defined in `config.toml`).
- The command takes one **required** argument: `name` (what to call the model, e.g. `Castle`).
- Everything afterwards is triggered by **ordinary chat messages inside the thread**. There is no prefix and no mention: the bot recognises building instructions by their grammar.

## Features

1. **Threaded Sessions**:
   - The command posts a short introduction and opens a public thread from it, named after the model.
   - Anyone who can see the thread can build in it. The session belongs to the thread, not to the person who started it.
   - Sessions expire after `session_ttl_hours` of inactivity, and are extended whenever someone builds.

2. **Silence By Default**:
   - A message is only treated as a command when it opens with a verb the language knows, or with a question word.
   - Anything else — "nice!", "I think the roof needs work", "brb" — produces **no reply, no reaction and no render**. The thread stays usable for ordinary conversation.
   - A sentence that *does* open with a known verb but fails to parse is reported, because the builder clearly meant something.
   - A sentence opening with a *question* word is different: "what do you think?" and "where is everyone?" look exactly like "what changed?" and "where is tower?", so a question the language cannot answer — or one naming something nobody has built — is answered with silence rather than an error. A question changes nothing either way.

3. **Collaborative Editing**:
   - Every accepted command becomes a revision in an append-only log, attributed to whoever sent it.
   - `Undo.`, `Redo.`, `Revert to revision 12.` and `Undo alice's last edit.` all work, the last of these leaving everyone else's edits in place.
   - Names are matched against the people who have actually edited this model, by display name or nickname; an unambiguous abbreviation (`Undo al's last edit.`) is enough, and `Undo my last edit.` needs no name at all. A name nobody in the thread answers to is reported rather than silently doing nothing.
   - Undo and redo survive a restart: where the session has been rewound to is stored alongside it, not baked into the log.
   - Because history is a log of operations rather than snapshots, `Who added roof?` and `What changed?` are answerable.

4. **Conversational References**:
   - `that`, `it`, `them`, `the last red brick`, `the highest brick`, `all red bricks` and `the selection` resolve against what has just been built.
   - `Call this wall.` names a group; `Name this point gate.` names a location. Both can be referred to afterwards.

5. **High-Level Structures**:
   - `Build a wall 10 studs long 6 bricks high.` expands server-side into individual bricks, laid in a running bond.
   - Walls, rows, towers, floors, boxes and stairs are supported, along with `Mirror` and `Duplicate`.
   - `Mirror the wall across north.` picks the axis to reflect in; `across the centre` names the pivot and keeps the default east-west axis. Mirroring turns each part to face the other way, which is a rotation rather than a true reflection — right for bricks and plates, approximate for slopes.
   - `Duplicate` steps the copy clear by the width of *what was copied*, not of the whole model, so duplicating one brick inside a large build puts it alongside.
   - Expansions are **not** black boxes: every brick they create is individually addressable afterwards, so `Paint all red bricks dark tan.` works on them normally. A whole structure is a single undo step.

6. **Rendering**:
   - After every accepted command the model is re-rendered and posted as a PNG.
   - Rendering uses real LDraw part geometry when the part library is installed (see [the runbook](../runbook.md)); until then it draws simple boxes and says so once.
   - A burst of commands coalesces into a single render, so several people building at once produce one picture rather than one each.

7. **Part Resolution**:
   - Everyday elements (`a red 2x4 brick`) resolve instantly from a built-in catalogue, with no lookup.
   - Anything unusual falls through to the same Rebrickable search that backs [`/part`](part.md), and the builder picks from a dropdown. The sentence that was waiting is then carried out with the chosen element, exactly as if the catalogue had known it. Anything else that changes the model in the meantime cancels the pending choice, since the sentence would no longer mean what its author meant.

8. **Export**:
   - The `[Export .io]` button, or the sentence `Export.`, produces a BrickLink Studio `.io` file.
   - The exported file contains only part *references*, never part geometry.

9. **Localization**:
   - Everything the bot *says* follows the guild's `locale`: the command name and argument, the
     intro embed, the buttons, every confirmation, answer and error. `src/build/report.rs` is the
     only place that calls `rust_i18n::t!` for session output, so the whole vocabulary lives in
     `command.build.*` in both locale bundles.
   - Parse failures and unsupported commands are localized down to the fragment: `Expectation`
     (`src/build/dsl/parser.rs`) and `Unsupported` (`src/build/lower.rs`) are closed sets whose
     phrasing lives under `command.build.expected.*` and `command.build.unsupported.*` rather than
     as English literals in the code.
   - **The language itself is not translated.** BrickTalk verbs stay English in every locale, so
     the help text and examples quote English sentences even in Finnish. Words echoed back from
     what the builder typed — `'that'`, a group name, a part query — appear as typed.

## Configuration
The command can be enabled or disabled globally or on a per-guild basis using the `config.toml` file. It is **off by default**, because it opens threads and listens to the conversation in them:

```toml
[commands.build]
enabled = true
session_ttl_hours = 24
render_width = 800
render_height = 600
supersample = 2
render_debounce_ms = 1200
enable_io_export = true

[[guilds]]
guild_id = 123456789
[guilds.commands.build]
enabled = false # Disables /build for this specific guild
```

The LDraw part library is configured separately, and is shared by every guild:

```toml
[ldraw]
data_dir = "data/ldraw"
library_url = "https://library.ldraw.org/library/updates/complete.zip"
max_files = 200
```

Install it once with `make fetch-ldraw`. See [ADR-003](../ADR-003-LDraw-Rendering-And-Studio-Export.md) for why it is fetched rather than bundled.

## Internal Implementation
Everything except the last item is pure and unit-tested, with no serenity, sqlx or reqwest types crossing into it:

- `src/build/dsl/` — the language: `lexer.rs` (tokens, multi-word colours, Discord noise stripping), `ast.rs`, `parser.rs` (the grammar and the silence policy).
- `src/build/lower.rs` — resolves references and locations against the model and emits operations.
- `src/build/expand.rs` — expands high-level structures into ordinary placements.
- `src/build/{model,ops,units}.rs` — the model, the operation log, and LDraw units.
- `src/build/{parts,queries,report,scene}.rs` — the part catalogue, questions, localized output, and the model-to-triangles step.
- `src/build/export/` — LDraw text and the `.io` container.
- `src/ldraw/` and `src/render/` — part geometry and the software rasterizer.
- `src/build/dispatch.rs` — `process_message`, the single pure entry point the Discord layer calls.
- `src/build/session.rs` — the only part that touches Discord: thread creation, persistence, the render loop and the part-choice dropdown.

Storage is `build_sessions` and `build_revisions` (`migrations/0002_build_sessions.sql`, `migrations/0003_build_session_state.sql`), accessed through `src/db/build.rs`. Only the operation log is stored; the model is a fold over it. Where undo and redo have left the session is session state (`build_sessions.cursor`), separate from the per-revision `skipped` flag that `Undo alice's last edit.` sets — conflating the two makes redo impossible to express.

Each session's read-interpret-write cycle is serialised by a per-session lock and committed in one transaction, because two people building in the same second is the expected case rather than a rare one.

## Appendix: The BrickTalk language

Sentences end with a full stop (or a newline). Several may appear in one message and are applied in order.

### Verbs

| Verb | Example |
|---|---|
| `add` / `put` / `place` | `Add a red 2x4 brick at origin.` |
| `move` / `shift` | `Move it one stud east.` |
| `raise` / `lower` | `Raise by one plate.` |
| `rotate` / `turn` | `Rotate 90 degrees.` `Turn it upside down.` Angles must be whole quarter turns; `Rotate 45 degrees.` is rejected rather than rounded. |
| `remove` / `delete` | `Remove the last brick.` |
| `replace` / `swap` | `Replace all red bricks with dark red.` |
| `paint` / `colour` | `Paint the selection dark tan.` |
| `build` / `make` | `Build a wall 10 studs long 6 bricks high.` |
| `duplicate` / `mirror` | `Mirror everything.` `Duplicate tower east 4 studs.` |
| `select` | `Select all red bricks.` |
| `group` / `call` / `name` | `Call this wall.` `Name this point gate.` |
| `undo` / `redo` / `revert` | `Undo.` `Undo alice's last edit.` `Revert to revision 12.` |
| `count` / `find` / `show` | `Count red bricks.` `Find collisions.` `Show unsupported bricks.` |
| `export` / `render` / `help` / `finish` | `Export.` `Help.` |

### Parts

`[count] [colour] [size] [kind]`, in any sensible order — `a red 2x4 brick`, `3 tan plates`, `2x2 tile`. A bare four-digit number is read as a part number: `Add 3001 at origin.`

Colours are LDraw colour names: `red`, `dark tan`, `light bluish gray`, `trans clear`. American and British spellings both work. Plain `grey` means the modern bluish grey, not the 1990s colour.

### Locations

| Form | Example |
|---|---|
| `at origin` | `Add a gray 8x8 plate at origin.` |
| `at <x> <y> <z>` | `Add a brick at 20 0 40.` |
| `at <landmark>` | `Add a brick at gate.` |
| `on` / `on top of` | `Put a blue plate on top of that.` |
| `under` | `Add a black tile under the roof.` |
| `<direction> of` | `Add a white 1x2 plate north of the window.` |

With no location, a part rests on top of whatever was last touched — which is how a conversation naturally stacks bricks.

### Units and directions

One stud is 20 LDU, one plate 8, one brick 24. Directions are `north`, `south`, `east`, `west`, `up`, `down`. A bare count means studs horizontally but bricks vertically, so `Move it east 2.` is two studs while `Move it up 2.` is two bricks. Say the unit to override: `Raise by one plate.`

### Comments

Lines beginning `Note:` or `TODO:` are acknowledged with a reaction and otherwise ignored.

### Example session

```
Alice: Add a light bluish gray 8x8 plate at origin.
Bob:   Build a wall 8 studs long 4 bricks high.
Carol: Mirror the wall across the centre.
Bob:   Build a tower 6 bricks high.
Alice: Paint the highest brick red.
Carol: How tall is it?
Bob:   Export.
```
