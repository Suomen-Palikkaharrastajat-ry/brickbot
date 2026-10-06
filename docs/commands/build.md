# Build Command

## Overview
The `/build` command starts a collaborative LEGO building session in a Discord thread. It is the only slash command in the feature: once the thread exists, the bot listens to the conversation in it and interprets plain sentences as building instructions, re-rendering the model after every accepted command. The result can be exported as a BrickLink Studio file.

The language it understands is a controlled natural language — English-like, quick to type, and deterministic. It is parsed locally by a hand-written recursive-descent parser; no external language model is involved, which keeps interpretation reproducible and matches the privacy stance in [the interactions overview](../interactions/README.md).

## Trigger
- Triggered as `/build` (or `/rakenna` depending on the `locale` defined in `config.toml`).
- The command takes one **required** argument: `name` (what to call the model, e.g. `Castle`).
- It also takes one **optional** argument: `size`, overriding how big *this* model may grow —
  see [Sizing a model](#sizing-a-model). Without it, the session falls back to the guild's
  configured `max_model_size`.
- Everything afterwards is triggered by **ordinary chat messages inside the thread**. There is no prefix and no mention: the bot recognises building instructions by their grammar.

## Sizing a model

Every model is capped to a box, checked after each command against the model's whole
bounding box — not just what one command adds, so many small edits cannot creep a model past
the limit any more easily than one big one. A command that would push the model past its box is
rejected with a message rather than silently clamped or repositioned. Editing an
already-oversized model (say, one opened under a looser `size`) is still allowed, as long as the
edit does not make the model any bigger.

The `size` argument on `/build` sets this box for the session, in three shapes:

- `10` — a cube: 10 studs on X and Z, 10 bricks on Y.
- `8x12` — a footprint, 8 studs by 12 studs, with the height set to the larger of the two
  (12 bricks here) since a rectangle names nothing to be tall by.
- `8x12x6` — width, depth and height given outright: 8 studs, 12 studs, 6 bricks.

Left out, the session uses the guild's configured `max_model_size` (a cube, default 10). The
size is fixed for the life of the session — it cannot be changed after `/build` opens the
thread, since a shrink would leave earlier revisions validated against a limit that no longer
applies.

## Features

1. **Threaded Sessions**:
   - The command posts a short introduction and opens a public thread from it, named after the model.
   - The thread opens with a starter message carrying **the same button row every picture
     carries**, so `[Draw…]` is there before anything has been built. Without it the buttons
     arrived only under a render, and a builder whose first sentence did not land never learned
     that drawing was an option at all. Every handler copes with an empty model.
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

6. **Drawings**:
   - A message can carry an ASCII drawing in a fenced code block: a header sentence naming the
     view, then a legend and a grid. See [Drawings](#drawings) for the full syntax.
   - One cell is one stud. A symbol repeated over a rectangle the size of its part's footprint
     becomes **one** part, so `bbbb` with `b = red 1x4 brick` is a single brick rather than four
     in a heap.
   - `Show top view.` draws the model back **in the same syntax**, so the answer can be pasted
     straight into the thread to rebuild it. That round trip is how the syntax is meant to be
     discovered, and it needs neither the LDraw library nor the renderer.
   - Like a structure, a whole drawing is one undo step while every part in it stays
     individually addressable afterwards.

7. **Drafting Privately**:
   - The `[Draw…]` button on any render opens a private form — view, legend, drawing, and an
     optional location — and shows a preview **only you can see**.
   - The preview is the real interpreter running against a copy of the session: the same
     replies, the same errors, nothing written. `[Edit]` reopens the form as you left it,
     `[Apply]` commits it and posts publicly, `[Discard]` throws it away.
   - The form opens **prefilled with the current model**, so the easiest way to pick parts that
     exist is to start from the ones already in the build and edit them.
   - `[Apply]` **refuses** if anyone else has built since the preview: what you saw must be
     exactly what lands. The draft is kept, and `[Preview again]` re-runs it against the model
     as it now stands.
   - The form holds **drawings only** — ordinary sentences like `Paint the highest brick red.`
     stay on the public path. A Discord form can hold nothing but text boxes, which is also why
     parts and colours are picked either side of it rather than inside it.

8. **Rendering**:
   - After every accepted command the model is re-rendered and posted as a PNG.
   - Rendering uses real LDraw part geometry when the part library is installed (see [the runbook](../runbook.md)); until then it draws simple boxes and says so once.
   - A burst of commands coalesces into a single render, so several people building at once produce one picture rather than one each.

9. **Part Resolution**:
   - Everyday elements (`a red 2x4 brick`) resolve instantly from a built-in catalogue, with no lookup.
   - Anything unusual falls through to the same Rebrickable search that backs [`/part`](part.md), and the builder picks from a dropdown. The sentence that was waiting is then carried out with the chosen element, exactly as if the catalogue had known it. Anything else that changes the model in the meantime cancels the pending choice, since the sentence would no longer mean what its author meant.

10. **Export**:
   - The `[Export .io]` button, or the sentence `Export.`, produces a BrickLink Studio `.io` file.
   - The exported file contains only part *references*, never part geometry.

11. **Localization**:
   - Everything the bot *says* follows the guild's `locale`: the command name and argument, the
     intro embed, the buttons, every confirmation, answer and error. `src/build/report.rs` is the
     only place that calls `rust_i18n::t!` for session output, so the whole vocabulary lives in
     `command.build.*` in both locale bundles.
   - Parse failures and unsupported commands are localized down to the fragment: `Expectation`
     (`src/build/dsl/parser/`) and `Unsupported` (`src/build/lower.rs`) are closed sets whose
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
max_model_size = 10

[[guilds]]
guild_id = 123456789
[guilds.commands.build]
enabled = false # Disables /build for this specific guild
```

`max_model_size` is the size cap a session gets when `/build` is not given its own `size`
argument: a cube of this many studs on X/Z and bricks on Y. See
[Sizing a model](#sizing-a-model) for how a session can override it.

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

- `src/build/dsl/` — the language: `block.rs` (lifting fenced drawings out before anything else reads them), `lexer.rs` (tokens, multi-word colours, Discord noise stripping), `ast.rs`, `parser/` (the grammar and the silence policy).
- `src/build/lower.rs` — resolves references and locations against the model and emits operations.
- `src/build/expand.rs` — expands high-level structures into ordinary placements.
- `src/build/grid.rs` — gathers a drawing's cells into whole parts and places them.
- `src/build/textview.rs` — draws the model back as a grid, in the syntax a drawing is typed in.
- `src/build/draft.rs` — the private form's fields, the text they assemble into, and the preview. Pure: a preview is `process_message` against a clone of the history with `persist` skipped, so the preview and the eventual apply cannot disagree.
- `src/build/{model,ops,units}.rs` — the model, the operation log, and LDraw units.
- `src/build/{parts,queries,report,scene}.rs` — the part catalogue, questions, localized output, and the model-to-triangles step.
- `crates/brick-model/src/export/` — LDraw text and the `.io` container.
- `crates/brick-render/src/{ldraw,render}/` — part geometry and the software rasterizer.
- `src/build/dispatch.rs` — `process_message`, the single pure entry point the Discord layer calls.
- `src/build/session/` — the only part that touches Discord: thread creation, persistence, the render loop and the part-choice dropdown.
- `src/build/debug_log.rs` — the optional `--build-debug-log` trace described below.

Storage is `build_sessions` and `build_revisions` (`migrations/0002_build_sessions.sql`, `migrations/0003_build_session_state.sql`), accessed through `src/db/build.rs`. Only the operation log is stored; the model is a fold over it. Where undo and redo have left the session is session state (`build_sessions.cursor`), separate from the per-revision `skipped` flag that `Undo alice's last edit.` sets — conflating the two makes redo impossible to express.

Each session's read-interpret-write cycle is serialised by a per-session lock and committed in one transaction, because two people building in the same second is the expected case rather than a rare one.

## Debugging & Grammar Iteration

Running the bot with `--build-debug-log <path>` appends a JSONL line for every message any build thread receives, whether or not it did anything — the raw input needed to see where BrickTalk's grammar or silence policy falls short in real conversation. Off by default; not a data-collection feature, just a development aid, and a write failure never interrupts building.

Each line:

```json
{"ts":"2026-07-31T12:00:00Z","session_id":"...","channel_id":123,"author_id":"456","locale":"en-US","message":"Add a red 2x4 brick at origin.","classification":"ok","reactions":["✅"],"replies":["Added a red 2x4 brick."],"ops_applied":1,"pending_lookup":false}
```

`classification` is one of:
- `ignored` — the silence policy read it as ordinary chat before parsing ever reached the model. Skim these for sentences that plainly meant a command but didn't open with a known verb or question word.
- `silent` — parsed further (typically a question) but resolved to nothing worth reporting.
- `ok` — applied, at least in part.
- `problem` — stopped on a parse or lowering error; `replies` carries what was reported.

`problem` and `ignored` lines are the ones worth reviewing when deciding what to extend in `src/build/dsl/`.

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
| `build` / `make` | `Build a wall 10 studs long 6 bricks high.` `Build from top at origin:` followed by a drawing — see [Drawings](#drawings). |
| `duplicate` / `mirror` | `Mirror everything.` `Duplicate tower east 4 studs.` |
| `select` | `Select all red bricks.` |
| `group` / `call` / `name` | `Call this wall.` `Name this point gate.` |
| `undo` / `redo` / `revert` | `Undo.` `Undo alice's last edit.` `Revert to revision 12.` |
| `count` / `find` / `show` | `Count red bricks.` `Find collisions.` `Show unsupported bricks.` `Show top view.` |
| `export` / `render` / `help` / `commands` / `finish` | `Export.` `Help.` `Help select.` |

`Help.` alone shows this table's overview; `Help <verb>.` (e.g. `Help select.`) shows that verb's own syntax and examples, localized like everything else the bot says. It covers every verb in the table above by its canonical name — a synonym still resolves to it, so `Help put.` answers the same as `Help add.`. `Attach`, `Fill` and `Repeat` are lexer-level aliases rather than verbs with syntax of their own, so a topic that names one of those, or one the parser doesn't recognise as a verb at all, falls back to the general overview instead of erroring.

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

`under` needs somewhere to go: asking for something under a part that is already on the ground is
answered rather than silently turned into "on top of", which would be the opposite of what was said.

### Help

`Help.` shows the overview: how to say it, how to draw it, how to ask and fix, and the list of
topics. `Help <topic>.` (e.g. `Help select.`) shows one topic's syntax and examples, localized
like everything else the bot says.

**A topic is not always a verb.** Alongside every verb in the table above there are four named
topics — `drawings`, `locations`, `parts` and `buttons` — because those are the things a builder
asks about by name, and drawings in particular are the one thing nobody can guess the shape of.
`Help drawings.` carries a complete legend and grid meant to be copied straight out of Discord.

- Synonyms resolve to their canonical verb, so `Help put.` answers the same as `Help add.`.
  `Attach`, `Fill` and `Repeat` are lexer-level aliases rather than verbs with syntax of their
  own and fold into `add` and `duplicate`.
- Named topics are resolved **before** verbs, which is the point of them: `draw` is a synonym of
  `render` and `colour` of `paint`, so `Help draw.` used to answer "render the model again" and
  `Help colour.` used to answer with paint's syntax. Both now answer the question that was asked,
  while `Help render.` and `Help paint.` still reach the verbs.
- The topic is read from the **sentence as typed** rather than from the token stream, since
  tokenizing has already collapsed `draw` into `Verb::Render`. The first word after `help` that
  names a topic wins, so `Help me with drawings.` lands where it was meant to.
- A word that is not a topic at all falls back to the general overview instead of erroring:
  asking for help wrong is still asking for help.
- The topic list itself lives in one place, `command.build.help_topics`, which the general help
  interpolates. A test round-trips every word on that list through `HelpTopic::parse` and checks
  the general help names it, so what `Help.` advertises and what `Help <topic>.` answers cannot
  drift apart.

### Naming a part without saying where

Typing just a part — `2x2 brick`, `2 red 2x4 bricks`, `3 2x2 bricks stacked vertically` — asks
where it could go rather than saying. The bot works out up to four legal positions, draws them as
one picture, and puts a numbered button under each: one click places it.

- A size (`2x2`) or a kind (`brick`, `plate`, `tile`, `slope`) has to be in there. A bare colour or
  count is how people talk — `red`, `2 beers` — and stays silent, as does anything with words left
  over. Half-understanding a sentence is the one thing the silence policy cannot afford.
- Several of a part with nothing said about how they sit offers the arrangements too, so `2 2x2
  bricks` competes "stacked" against "side by side" in the same list of four. Saying `stacked` or
  `in a row` settles it.
- The positions offered are: on top of whatever was last touched (which is what a full sentence
  does today, and is always offered first), alongside it, on top of another exposed surface, and on
  the ground clear of everything — where a second structure would start.
- **Nothing is built until a number is pressed.** If someone else builds in the meantime the offer
  is refused rather than misplaced, because what you were shown is no longer what would land.
- `Draw one of these instead…` opens the drawing form at the chosen position, so a proposed brick
  can grow into a whole block without retyping where it goes.

Every offer is legal by the rules below — the proposals are put on the grid and settled by the same
code a typed-out sentence goes through, so the dialog cannot offer somewhere a part could not go.

A **drawing that never says where it goes** asks the same question: `Build from top:` with a legend
and a grid, and no `at …`, offers positions for the whole block. A drawing that does say where it
goes is unaffected. Inside the private drawing form, `[Place…]` does the same for whatever is being
drafted.

Because a chat message carries no interaction token, the proposal is posted **in the thread** rather
than privately — the same reason the part-disambiguation dropdown is public. Only the person who
asked can act on it.

### Legal connections

Every command is corrected before it is carried out, so the model can only ever contain parts that
would really go together. Three rules, all in `src/build/legal.rs`:

- **On the grid.** A part's edges land on whole studs (20 LDU) and its base on a whole plate (8 LDU).
  `Add a brick at 3 5 7.` is a brick at the origin; `Move it east 2 plates.` moves it a stud, because
  a plate is a vertical measure and 16 LDU sideways attaches to nothing. Note that an odd-width part
  is centred *half* a stud off the grid lines, since it is the edges that have to line up.
- **Nothing inside anything else.** A part put where another already is comes to rest on top of it
  instead of sharing its volume. Parts may touch face to face; that is what stacking is.
- **Nothing floating, and nothing underground.** A part in mid-air comes down to the nearest thing
  below it, or to the ground. A whole structure settles as one body and keeps its shape, so it may
  overhang what it rests on — real builds do.

The bot says what it corrected (`Nudged 1 part onto the stud grid.`) and stays quiet when there was
nothing to correct, which is the usual case. Very rarely there is nowhere legal for a part at all,
and then it says so instead of guessing.

`Raise` and `Lower` are the deliberate exception: they are corrected onto the grid and out of
collisions, but a part you lifted on purpose stays lifted rather than dropping straight back down.
`Show unsupported bricks.` still finds it.

Corrections apply to *new* commands only. A session recorded before any of this existed replays
exactly as it always did, so old threads redraw unchanged.


### Drawings

Some things are far easier to draw than to describe. A message can carry an ASCII drawing: a
header sentence naming the view, then a fenced code block holding a legend and a grid.

````
Build from top at origin:
```
b = blue 1x1 brick
r = red 1x1 brick

.bb.
rbbr
```
````

- **The header is an ordinary sentence** and sits *outside* the fence, so every location form
  above works on it: `Build from top at origin:`, `Build from north at 20 0 40:`,
  `Build from east at gate:`. With no location, a drawing starts on top of whatever was last
  touched, like anything else. `from` is optional (`Build top view:`) and a trailing `:` is
  ignored.
- **The fence holds nothing but the legend and the grid.** It must be a triple backtick fence;
  single backticks are still treated as inline code and dropped.
- **A legend line is `<one character> = <part>`**, recognised wherever it appears — above,
  below or among the drawing. The part is described exactly as in `Add …`, so `blue 1x1 brick`,
  `3 tan plates` and a bare part number all work. A key of more than one character is reported.
- **`.` and a space are empty cells.** Everything else must be in the legend.
- **A blank line starts a new block** — the next layer up in a plan view, the next slice back in
  an elevation.

#### One cell, one stud

A cell is one stud. A symbol repeated over a rectangle matching its part's footprint becomes
**one** part, so `bbbb` with `b = red 1x4 brick` is a single brick rather than four in a heap.

The footprint is tried as written and then turned a quarter, and the first that fits wins,
scanning from the top left. That makes the spelling in the legend meaningful: a 2x2 field of
`b` tiles into two dominoes running back if the legend says `1x2 brick`, and two running across
if it says `2x1 brick`. **Parts are laid the way you drew them.**

Cells that cannot be gathered into a whole part are reported with their position, and nothing is
placed — half a drawing would leave you undoing the rest by hand.

#### Views

| View | Columns run | Rows run | Each block is |
|---|---|---|---|
| `top` (`above`, `plan`) | east | south | the next layer up |
| `bottom` (`below`, `underside`) | east | north | the next layer up |
| `north` | east | downward | the next slice back |
| `south` | west | downward | the next slice back |
| `east` | south | downward | the next slice back |
| `west` | north | downward | the next slice back |

An elevation sees only the face turned towards it, so parts are never gathered across two rows:
two 1x1 bricks drawn one above the other really are two bricks. Its rows are courses of real
parts, so a plate course is 8 LDU and a brick course 24 — the drawing stacks by what is actually
in it rather than at a fixed pitch. Plan-view layers likewise rest on the tallest part below.

#### Reading a model back

`Show top view.` draws the model as a grid **in this same syntax**, so the answer can be pasted
back into the thread to rebuild it. `Show the north view.`, `Show the view from the east.` and
`Show top view layer 2.` all work. Symbols are assigned from the model's own parts list, so a
colour usually gets its initial and the output is identical every time for the same model.

The reply is capped to one Discord message: whole layers are dropped from the end rather than
cutting a drawing off mid-row, and the bot says how many it left out.

Round trips are exact for stud-aligned models built from catalogue parts. Parts sitting off the
stud grid snap to the nearest cell, and a part with no known footprint cannot be drawn at all.

### Drafting a drawing privately

A build thread is public, so every attempt at a drawing is seen by everyone. The `[Draw…]`
button under any render opens a private form instead:

| Field | What goes in it |
|---|---|
| View | `top`, `north`, `east`, … |
| Legend | one `b = blue 1x1 brick` per line |
| Drawing | the grid; `.` is an empty stud, a blank line starts the next layer |
| Where to put it | optional — any location form from the table above |

Submitting shows a preview **only you can see**: the drawing as it would stand, plus whatever
the interpreter had to say about it. Nothing is written to the model. `[Edit]` reopens the form
as you left it, so a legend with a bad colour or a size that does not exist can be fixed without
anyone watching.

The form opens **prefilled with the current model**. Editing what is already built is the
simplest way to name parts and colours that certainly exist — the legend it opens with is one
the bot itself wrote.

`[Apply]` commits the drawing and posts it publicly, exactly as if it had been typed into the
thread. It **refuses** if anyone else has built in the meantime: the preview was taken against a
particular revision, and what you looked at must be what lands. The draft survives, and
`[Preview again]` re-runs it against the model as it now stands.

Two limits worth knowing. The form holds **drawings only** — ordinary sentences stay on the
public path. And a Discord form can contain nothing but text boxes, so there is no dropdown
inside it; parts and colours are chosen before or after, never within.

### Choosing parts and colours

Discord's only typed-ahead is slash-command autocomplete, and it cannot run inside a form — a
Discord modal holds nothing but text boxes. So suggestions appear where they can: on the `/part`
command's argument, which now autocompletes as you type.

Suggestions never wait for the network. Autocomplete fires on every keystroke with a
three-second deadline, so it is answered from two local sources:

- the **built-in catalogue** of everyday sizes, compiled into the binary, which works on a cold
  start and with no API key at all;
- the **sticky cache** (`data/cache.db`), which remembers every part lookup the bot has ever
  made.

A query the cache does not know is answered with whatever is on hand, and quietly fetched in the
background — so the keystroke that misses costs nothing and the next one is fast. Because
ordinary `/part` searches fill the same cache, suggestions get better the more the guild uses
the bot, and they survive restarts.

The cache also answers queries nobody has typed before: it indexes part *names*, so `wind`
finds `Windscreen 3 x 4 x 1.3` if any earlier search happened to mention it.

#### Adding to a legend without typing the line

A form cannot hold a dropdown, so the legend is edited from outside it. Two routes, and they
reach different things:

| | Reaches | For |
|---|---|---|
| `/build-legend part: colour:` | the **whole** palette and everything ever looked up | when you know roughly what you want |
| `[Insert…]` on the preview | the 74 colours and the 38 catalogue parts, a page at a time | when you don't, and want to look |

Both append a line to your open drawing and show the preview again; `[Edit]` then reopens the
form with the line in it. `/build-legend` opens a drawing for you if you have none, so it works
as a first step and not only as an edit. Neither needs a symbol — one is chosen for you,
preferring the colour's own initial, so a legend reads the way you would have written it. Pass
`symbol:` to choose it yourself.

`[Insert…]` asks for a colour first and then a part, because a colour on its own is not a legend
line. "No colour" is offered on every page and gives a line that takes the drawing's default.

Everything in `data/cache.db` is derived and disposable — deleting the file is a clean rebuild.
See `[parts_cache]` in `config.toml.example` for the expiry settings; part records never expire,
searches last 30 days, and a search that found nothing lasts an hour.

### When a part or colour is not recognised

The catalogue holds the everyday elements, not every size. A size that does not exist is
answered with the ones that do:

```
Add a 1x7 brick.
→ I don't have 'brick 1 x 7'. Did you mean 1x6 brick, 1x8 brick?
```

A symbol missing from a drawing's legend is answered with the symbols that are in it, so a typo
in a grid does not require re-reading the whole legend to spot.

### Units and directions

One stud is 20 LDU, one plate 8, one brick 24. Directions are `north`, `south`, `east`, `west`, `up`, `down`. A bare count means studs horizontally but bricks vertically, so `Move it east 2.` is two studs while `Move it up 2.` is two bricks. Say the unit to override: `Raise by one plate.`

### Comments

Lines beginning `Note:` or `TODO:` are acknowledged with a reaction and otherwise ignored.

### Example session

````
Alice: Add a light bluish gray 8x8 plate at origin.
Bob:   Build a wall 8 studs long 4 bricks high.
Carol: Mirror the wall across the centre.
Bob:   Build a tower 6 bricks high.
Alice: Paint the highest brick red.
Carol: How tall is it?
Alice: Build from top:
       ```
       t = tan 1x1 tile
       
       .tt.
       tttt
       ```
Bob:   Show top view.
Bob:   Export.
````
