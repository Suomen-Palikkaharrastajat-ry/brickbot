# ADR-003: LDraw Rendering and BrickLink Studio Export

## Status
Accepted

## Date
2026-07-28

## Context

The [`/build`](commands/build.md) command lets several members edit a shared 3D LEGO model by typing sentences in a Discord thread. Two capabilities it needs did not exist anywhere in this codebase:

1. **Rendering.** After every accepted command the bot must post a picture of the model. There was no graphics, geometry or image code in `src/`, and no image crate in `Cargo.toml`.
2. **Export.** The model must be openable in BrickLink Studio, the tool the community actually builds in. There was no model-format code either.

Three constraints shaped the answer:

- The bot runs as a small static binary built by Nix, on a host with no GPU and no display server.
- `AGENTS.md` requires business logic to be decoupled from external API surfaces and to be unit-testable, which rules out anything that can only be verified by looking at it.
- The LDraw part library is a third-party corpus of thousands of files with its own licence terms, including files explicitly marked *Not redistributable*.

## Decision

### 1. A software rasterizer, not a GPU or a hosted service

Renders are produced by a z-buffered scan-line rasterizer written in Rust (`src/render/`), running on a blocking thread pool.

A GPU path (`wgpu`) was rejected: there is no GPU on the host, software fallbacks add a large dependency tree, and headless GL in a static MUSL build is a source of ongoing pain. A hosted rendering service was rejected because it would put the feature's core loop behind a third-party availability and privacy boundary for no benefit at this scale.

The camera is **orthographic** and auto-framed from the model's bounding box at a fixed three-quarter angle. That needs no field-of-view or near/far tuning, never drops a far corner of a large model, reads like the isometric drawings in an instruction booklet, and — because framing depends only on the bounding box — makes renders deterministic.

Shading is flat and two-sided. LDraw's BFC winding metadata is subtle and inconsistently applied across the library, and flattening negates the Y axis (LDraw is −Y up, the renderer +Y up), which mirrors every part and would invert every normal. Two-sided lighting removes that entire class of bug at the cost of some interior faces showing through open geometry, which the depth buffer largely hides on closed brick shapes.

### 2. The LDraw library is installed, not embedded and not fetched per render

`brickbot --fetch-ldraw` (or `make fetch-ldraw`) downloads the library once into a configured `data_dir`. Renders read it from disk.

Embedding the library in the binary was rejected: it is tens of megabytes, it would bloat the Nix build, and it would make us a redistributor of a corpus we merely pass through. Fetching part files lazily over HTTP during a render was implemented first and then removed: it made the first render of every session slow and unpredictable, and put the bot's core loop at the mercy of a third-party server being up.

Until the library is installed the feature still works — it renders every placement as a plain box and says so once in the thread. That keeps a fresh deployment usable rather than broken.

### 3. `.io` export with a hand-rolled ZipCrypto writer

A BrickLink Studio `.io` file is a ZIP archive encrypted with the legacy PKWARE stream cipher and the fixed, publicly known password `soho0909`, containing `model.ldr`, `model2.ldr`, `.INFO` and `THUMBNAIL.PNG`.

The `zip` crate can *read* ZipCrypto but cannot write it, so `src/build/export/` writes the container by hand: a 256-entry CRC-32 table, the three-word key schedule, and the handful of ZIP records needed. All entries are **stored** rather than deflated — an LDraw file for five hundred bricks is around 40 KB — which removes compression from the problem entirely.

`crc32fast` is deliberately not used: the cipher needs the raw, non-inverted per-byte CRC update, which that crate's hasher does not expose. The same local table serves the archive's entry checksums.

Exports are byte-for-byte reproducible: timestamps are fixed and the encryption header's filler is derived from the content rather than randomly, so an export can be golden-tested.

## Architecture

```
model (LDraw coordinates, integers)
  │
  ├─ src/build/export/ ──────────────► .ldr text ──► ZipCrypto ──► .io
  │
  └─ src/build/scene.rs
       │  box mode ─────────────────► triangles
       │  mesh mode
       │    └─ src/ldraw/resolve.rs ─► data_dir on disk
       │         └─ parser → geometry (flatten, negate Y)
       ▼
     src/render/  camera → rasterizer → shader → PNG
```

The model is stored **natively in LDraw coordinates** (right-handed, −Y up, integer LDU). Only the renderer flips Y. That makes `.ldr` export a direct dump with no transform, eliminating a class of mirror-image bugs, and integer coordinates mean no floating-point drift accumulates over a long session — which is what makes "the same command always produces the same model from the same revision" true in practice.

Two caches make repeated renders cheap: parsed part files, and flattened meshes keyed by part name only. Meshes keep LDraw's `inherit` colour code, so one cached mesh serves every colour the part is used in. Without the mesh cache a 300-brick model would re-flatten `3001` and its ten sub-files three hundred times per render, which would dominate everything else.

## Consequences

### Positive
- No GPU, no display server, no external rendering service, and no runtime dependency on a third-party host during a render.
- Renders are deterministic, so the same model always produces the same image.
- Every stage except the disk read and the Discord call is a pure function, and is unit-tested. The `.io` container is verified against an independent implementation by having the `zip` crate decrypt our own output.
- The feature degrades rather than fails when the part library is missing.
- One new runtime dependency for rendering (`png`) and one for extraction (`zip`), both pure Rust, so the static Nix build is unaffected.

### Negative
- An operator must run `make fetch-ldraw` before renders show real part shapes. Without it the pictures are boxes.
- Flat two-sided shading with no edge lines is visibly simpler than a proper LDraw viewer. Adjacent bricks of the same colour have no visible seams.
- Maintaining a hand-written ZIP and cipher is a small ongoing cost, and `.INFO`'s real structure is undocumented — the blob we write is a plausible guess. `enable_io_export = false` falls back to a plain `.ldr`, which Studio imports natively.
- Rendering is CPU work on the bot's host, bounded by `spawn_blocking` and a process-wide semaphore of two.

### Licensing
This is the reason several of the decisions above look the way they do.

- We **do not embed** the LDraw library in the binary and **do not redistribute** it. It is installed by the operator into a directory they control, from a URL they configure.
- Files whose header marks them *Not redistributable* are detected during extraction and **never written to disk at all**, so they cannot reach a render or an export. This mirrors the check in the organisation's [brick-frame](https://github.com/Suomen-Palikkaharrastajat-ry/brick-frame) sync script.
- An exported `.io` contains only part **references** (`1 4 0 0 0 1 0 0 0 1 0 0 0 1 3001.dat`), never part geometry. Opening it requires the recipient's own copy of the library, exactly as any Studio or LDraw file does.

## Final Recommendation

Adopt the software rasterizer and the disk-installed library. The rendering quality is deliberately modest and can be improved incrementally — edge lines, smoothed normals, anti-aliasing — without changing any of the interfaces above.

Keep `enable_io_export` until one exported file has been opened in real BrickLink Studio. The container and cipher are proven against the `zip` crate, but `.INFO` is a guess, and the flag is the cheap way to fall back if Studio disagrees.
