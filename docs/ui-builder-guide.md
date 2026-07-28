# Mosaic — UI Builder's Guide

*A handoff for the person building Mosaic's UI. The concept, the vocabulary, the one rule that
makes everything trustworthy, the three jobs the UI does, the screens to build, and the exact
browser SDK + server API to build on. Every name and endpoint here is pulled from the shipped
code.*

---

## 1 · What Mosaic is

Mosaic turns an input — an image today, audio too — into a grid of characters or coloured
tiles, using a **style written by the community**.

Think of the classic "photo → ASCII art" toy, but open-ended: the mapping from picture to
glyphs isn't baked in. Anyone can author their own mapping (a **Facet**), share it, and everyone
else can run it on their own images. A Facet is real, untrusted code — so the whole platform is
built around running it *safely* and running it *identically everywhere*.

Two properties make it more than a toy, and both shape the UI you're building:

- **Untrusted styles run in a sandbox.** Community code can't touch the network, disk, or clock,
  and can't hang the page. The UI never has to trust a Facet it downloads.
- **Preview equals render.** The preview you compute locally in the browser is *bit-for-bit* the
  same as what the server produces. There's no "it looked different when I shared it."

The backend — sandbox, conformance gate, registry, render service — is done and tested. **You're
building the face of it.**

---

## 2 · The three names

Three words come up everywhere. They're a strict hierarchy — worth getting right in the UI copy
too.

| Name        | What it is |
|-------------|------------|
| **Mosaic**  | The platform / substrate. The product as a whole — the site, the registry, the account. When you name the product to a user, it's "Mosaic." |
| **Tessera** | An *engine*. It reads the raw input (pixels, audio) and turns it into per-cell **features** — numbers like brightness or edge strength. `tessera-ascii` handles images; `tessera-spectral` handles audio. The user picks one via the **engine** setting. |
| **Facet**   | A community-authored **style** — the creative unit users make, browse, and run. A pure function: features in, one glyph (and its cell) out. This is the noun your users care about most. |

Mental model: **Tessera** looks at the picture and measures it; the **Facet** decides what to
draw for each measurement; **Mosaic** is where all of that lives and is shared.

---

## 3 · The one rule: `preview == render`

The browser preview and the server's authoritative render are produced by the **same source
code**, compiled two ways (native + WebAssembly). Given the same input bytes, they return
identical output.

This is the platform's core promise, and it's a gift to the UI:

- **Preview is instant and free.** Compile, run, and paint entirely in the browser — no server
  round-trip while a user edits a style or drags a slider.
- **The server is the source of truth for anything shared.** When a user exports or shares a
  render, define it by the server's output (`POST /v1/render`). It will match the preview
  exactly, so there's no visible switch — it's just the authoritative copy.
- **Certificates make it checkable.** Every published Facet ships a certificate — golden
  `(features → glyphs)` samples the server recorded. The SDK can replay them in the browser and
  confirm your engine reproduces them. Use it as a trust badge / integrity check.

> **Don't break this.** Never invent a second code path that "roughly" renders a Facet. Always
> compile and run through the SDK. If a preview and a server render ever disagree, that's a bug
> worth stopping for — it's the one invariant everything else rests on.

---

## 4 · What a Facet is (two kinds)

Every Facet maps features → a glyph per cell. There are two ways to author one. **The UI's
authoring experience is about the second one.**

- **`kind: "wasm"` — compiled module.** A self-contained WebAssembly module, written by an
  advanced author in Rust/C/etc. Powerful, but not something you author in a text box. The UI
  mostly *runs* and *browses* these; it doesn't need an editor for them.
- **`kind: "program"` — DSL program (the friendly path).** A one-line expression in a tiny
  language — **Glint** — compiled to bytecode that runs on one shared interpreter. This is what
  your **in-browser editor** produces. Example:

  ```
  grad_mag > 0.6 ? glyph(floor(grad_dir), "-/|\\") : ramp(luma, " .:-=+*#%@")
  ```

Both kinds go through the same admission (the conformance gate → a certificate) and the same
registry lifecycle. From the UI's side the difference is: a **program** is authored in text and
has tweakable **parameters**; a **wasm** module is uploaded whole. A Facet's `artifact.kind`
field (`"wasm"` or `"program"`) tells you which you're looking at.

---

## 5 · Engines & output modes

The **engine** decides what input is read and what feature vocabulary a Facet sees (its
**stride** = features per cell). It's the top-level switch in both authoring and rendering.

| Engine             | Input        | Stride | Feature vocabulary                      | Needs a Facet? |
|--------------------|--------------|:------:|-----------------------------------------|:--------------:|
| `ascii`            | Image (RGBA) | 5      | `luma`, `grad_mag`, `grad_dir`, `u`, `v` | Yes            |
| `ascii-structural` | Image (RGBA) | 64     | L2 structural patch vocabulary          | Yes            |
| `spectral`         | Audio PCM    | 1      | `band_energy`                           | Yes            |
| `halfblock`        | Image (RGBA) | —      | — (pure colour, computed by the engine) | No             |

### Colour, three ways

Colour is computed by the *engine* from the source image (a deterministic mean), never by the
Facet — that's how colour stays inside `preview == render`.

- **Monochrome glyphs** — the default: a Facet's chosen character per cell, one colour.
- **Tinted glyphs** — the glyph render *plus* a per-cell colour. Ask `ascii` with `color: true`
  (server) or call `extractColors(...)` (browser) and paint each glyph in its cell's mean colour.
- **Half-block pixel-art** — no Facet at all. Each cell is `▀` (U+2580) with a foreground colour
  on top and a background colour below, doubling vertical resolution. This is the "coloured
  pixel-art" look.

> **Packed colour format.** Colours come back as `u32`, packed `r | g<<8 | b<<16 | a<<24`.
> To paint: `r = c & 255`, `g = (c>>8) & 255`, `b = (c>>16) & 255`, `a = (c>>>24) & 255`.

---

## 6 · What the UI does

Three jobs, all confirmed in scope. They map cleanly onto the SDK and API below.

- **Job A — Author & tweak.** A live editor: write a DSL style, pick an engine, drop an image,
  see the result update as you type. Sliders for the style's parameters, driven by its manifest —
  patch and re-run with no recompile.
- **Job B — Run published styles.** Browse the registry, pick a Facet someone shared, drop in
  your own image or sound, and render it. Download or share the result.
- **Job C — Coloured output.** Paint tinted glyphs and half-block pixel-art, not just monochrome
  text. A real canvas, real colour, saveable as an image.

---

## 7 · Screens to build

A suggested decomposition. Each screen lists what it contains and which SDK / API calls it leans
on (detailed in §8–9).

### 7.1 Studio — the authoring screen

The heart of the app. Where a user writes a style and watches it render live.

- **Engine picker** — `ascii` / `spectral` (the strides differ; the editor's feature names change
  with it).
- **Source editor** — a text box for the DSL. Show compile errors inline; the compiler returns a
  *byte offset* into the source, so you can point at the exact character.
- **Parameter panel** — one control (slider / number) per entry in the compiled style's
  *manifest*. Changing one calls `applyParams` and re-renders — no recompile, instant.
- **Input dropzone** — an image (decode to RGBA) or an audio clip (decode to PCM). Keep the raw
  bytes; that's what render wants.
- **Live preview** — a monospace grid or a colour canvas. Recompute on every edit; it's all local.
- **Publish** — name it, submit. It's certified on the spot; a success toast, then it's awaiting
  review.

### 7.2 Gallery — browse published styles

- A grid of **published** Facets: name, author, kind badge (`wasm` / `program`), and ideally a
  thumbnail (render each against a stock image).
- List from `GET /v1/facets`. Newest first.
- Click through to a detail / run screen.

### 7.3 Facet detail — run a published style

- Metadata + certificate. Fetch the style's bytes (`/wasm` or `/program` by kind) and run it
  locally, or call `POST /v1/render` by `id` for the authoritative copy.
- Input picker → render → output view (text or colour canvas) → download / copy / share.
- For a `program` Facet you can still surface its parameters as sliders (same manifest idea) so
  viewers can remix before running.

### 7.4 Review queue — moderators only

- List the pending queue: `GET /v1/facets?state=certified` (requires the moderator role).
- Approve or reject: `POST /v1/facets/{id}/moderate` with `{ "decision": "publish" | "reject" }`.

### 7.5 Identity

- Auth is a bearer token sent as `Authorization: Bearer <token>`. Confirm who you are with
  `GET /v1/whoami` → `{ id, roles }`.
- Gate the **Publish** action behind the `author` role and the **Review** screen behind
  `moderator`.

---

## 8 · Browser SDK

Everything for local preview lives in two packages: `@mosaic/facet-abi` (the sandboxed runner +
verifier) and the `mosaic-wasm` package (the DSL compiler + engine feature extraction + colour).
You never re-implement rendering — you call these.

### `mosaic-wasm` — compile, extract, compose, colour

| Call | Returns | Use |
|------|---------|-----|
| `compileDsl(engine, src, paramsJson)` | `{ program, manifestJson }` | Compile DSL text → bytecode + control manifest. Throws with a byte offset on a syntax error. |
| `extract_features(rgba, w, h, cols, cellAspect)` | `{ cols, rows, ncells, stride, data }` | Run the engine over an image → the per-cell feature buffer. Call `.free()` when done. |
| `compose(cols, rows, tokens)` | `string` | Glyph codepoints → the text grid (safe against untrusted glyphs). |
| `renderHalfblock(rgba, w, h, cols, cellAspect)` | `{ cols, rows, fg, bg, glyph }` | Coloured pixel-art, no Facet. `fg`/`bg` are packed `u32` per cell. |
| `extractColors(rgba, w, h, cols, cellAspect)` | `u32[]` | Per-cell mean colour, to tint a glyph render. |

`manifestJson` parses to `{ engine, stride, params: [{ name, value, index }] }` — one entry per
tweakable parameter. That's exactly your slider list.

### `@mosaic/facet-abi` — run & verify

| Call | Returns | Use |
|------|---------|-----|
| `compileFacet(bytes)` | `WebAssembly.Module` | Compile a wasm Facet — or the shared interpreter — once, then reuse. |
| `runFacetProgram(interp, program, features, ncells, stride)` | `Uint32Array` | Run a DSL program on the interpreter → glyph tokens. |
| `runFacetMap(module, features, ncells, stride)` | `Uint32Array` | Run a wasm Facet (the "gather" kind) → glyph tokens. |
| `runFacetMap2d(module, features, cols, rows, stride)` | `Uint32Array` | Run a "propagation" wasm Facet (needs 2-D grid shape). |
| `applyParams(program, values)` | `Uint8Array` | Patch a program's parameters in place → a re-runnable program. Your slider hook. |
| `runFacetProgramSandboxed(...)` & friends | `Promise` | The same runs, but in a timeout-metered Worker. Prefer these for *untrusted* Facets from the registry. |
| `verifyCertificate(bytes, cert)` · `verifyProgramCertificate(interp, program, cert)` | `Promise<void>` | Replay a Facet's certificate locally; resolves iff your engine reproduces the server's golden. A trust check. |

> **Copy from the working references.** These test files are runnable end-to-end examples of
> every path:
> - `crates/mosaic-wasm/test/authoring.test.ts` — compile → extract → run → compose + live controls
> - `crates/mosaic-wasm/test/color.test.ts` — half-block + tint
> - `packages/facet-abi/test/program.test.ts` — `applyParams`
> - `packages/facet-abi/test/program_cert.test.ts` — verify

> **Naming note.** A couple of `mosaic-wasm` calls surface as snake_case (`extract_features`,
> `compose`) because that's how the wasm bindings emit today; the rest are camelCase
> (`compileDsl`, `renderHalfblock`, `extractColors`). If you'd rather have a uniformly camelCase
> public SDK, that's a small, separate polish pass — flag it and it can be smoothed.

---

## 9 · Server API

The authoritative side: certify, render, and the registry. JSON in, JSON out. Base path `/v1`.
All errors share one shape:

```json
{ "error": { "code": "<stable slug>", "message": "<text>" } }
```

The `code` is machine-readable (e.g. `bad_magic`, `program_stride_mismatch`, `unknown_engine`),
so you can map it to a helpful editor message.

| Endpoint | Auth | Purpose |
|----------|------|---------|
| `POST /v1/render` | public | The authoritative render. Tagged by `engine`; facet is inline or by `id`. |
| `POST /v1/certify` | public | Check a wasm Facet against the gate, get its certificate. (Preview a submission.) |
| `GET /v1/facets` | public | List published Facets, newest first. `?state=certified` for the mod queue. |
| `POST /v1/facets` | author | Publish a Facet (wasm or program). Certifies, then stores as `certified`. |
| `GET /v1/facets/{id}` | visibility | A Facet's metadata + certificate. |
| `GET /v1/facets/{id}/wasm` | visibility | A wasm Facet's module bytes (`application/wasm`). |
| `GET /v1/facets/{id}/program` | visibility | A program Facet's bytecode (`application/octet-stream`). |
| `POST /v1/facets/{id}/moderate` | moderator | Approve or reject: `{ "decision": "publish" | "reject" }`. |
| `GET /v1/whoami` | token | Echo the caller's `{ id, roles }`. |

*"visibility" = public for published Facets; otherwise only the author or a moderator (a 404 to
anyone else).*

### Render — request shapes

```jsonc
// image → ASCII, optionally tinted. facet is inline OR { "id": "<facet>" }.
{ "engine": "ascii",
  "facet":  { "inline": "<base64 wasm>" },     // or { "id": "<published facet id>" }
  "input":  { "rgba": "<base64 RGBA8>", "width": 256, "height": 256 },
  "params": { "cols": 100, "cellAspect": 2.0, "color": false } }

// coloured pixel-art — no facet.
{ "engine": "halfblock",
  "input":  { "rgba": "<base64 RGBA8>", "width": 256, "height": 256 },
  "params": { "cols": 100, "cellAspect": 2.0 } }

// audio → spectrogram
{ "engine": "spectral",
  "facet":  { "inline": "<base64 wasm>" },
  "input":  { "pcm": "<base64 little-endian f32>", "sampleRate": 44100 },
  "params": { "bands": 64, "win": 1024, "hop": 256, "fmin": 40, "fmax": 16000 } }

// response — absent fields are omitted per mode
{ "cols": 100, "rows": 60,
  "text": "...",             // glyph modes
  "colors": [ /* u32 */ ],   // when color:true
  "glyph": 9600, "fg": [ ], "bg": [ ] }   // halfblock
```

### Publish — request & record

```jsonc
// a DSL program submission (the editor's output)
// POST /v1/facets   Authorization: Bearer <author token>
{ "name": "Sketchy Ink", "program": "<base64 bytecode>", "engine": "ascii" }
// (a wasm submission is { "name", "wasm" } instead)

// 201 → the stored record. `artifact` is tagged by `kind`.
{ "facet": {
    "id": "...", "name": "Sketchy Ink", "author": "alice",
    "state": "certified", "createdAt": 1730000000,
    "artifact": { "kind": "program", "engine": "ascii", "stride": 5,
                  "programSha256": "...", "certificate": { /* ... */ } } } }
```

Inputs are the raw authoritative bytes — RGBA8 for images, little-endian `f32` for PCM —
base64'd. That's the same data you previewed, so there's no decode drift. `cellAspect` ≈ 2.0
compensates for terminal cells being taller than wide.

---

## 10 · The authoring loop

The whole live-editor cycle, browser-local. This is the exact sequence in `authoring.test.ts` —
lift it.

```js
// once, on load: compile the shared interpreter
const interp = await compileFacet(interpBytes);

// on every edit: compile the author's text → bytecode + manifest
const compiled = wasm.compileDsl("ascii", src, JSON.stringify(params));
const manifest = JSON.parse(compiled.manifestJson);   // → build sliders from manifest.params

// measure the image, run the style, compose the grid
const fb = wasm.extract_features(rgba, w, h, cols, 2.0);
const tokens = runFacetProgram(interp, compiled.program,
                               Float32Array.from(fb.data), fb.ncells, fb.stride);
const text = wasm.compose(fb.cols, fb.rows, tokens);
fb.free();

// on a slider move: patch params, re-run — no recompile
const patched = applyParams(compiled.program, [newThreshold]);
const tokens2 = runFacetProgram(interp, patched, features, ncells, stride);
```

For colour, swap the render step: `renderHalfblock(...)` for pixel-art, or run the style and pair
each glyph with `extractColors(...)` for a tint. Paint to a canvas, cell by cell.

> **Sandbox anything from the registry.** When you run a Facet a *user* authored (your own editor,
> this session) the direct calls are fine. When you run a Facet *downloaded from the registry*,
> prefer the `...Sandboxed` variants — they run in a timeout-metered Worker so a hostile or
> runaway style can't freeze the tab.

---

## 11 · The Glint cheatsheet

*Glint* is the language a DSL Facet is written in. A single expression, evaluated once per cell.
Every value is a number; the final value is taken as a glyph codepoint. Useful for syntax
highlighting and autocomplete.

**Names & literals**

- **Features** (by engine): `luma`, `grad_mag`, `grad_dir`, `u`, `v` for `ascii`; `band_energy`
  for `spectral`. `u`/`v` are the cell's normalized centre position, each in `(0, 1)` — use them
  for gradients, vignettes, and spatial masks (e.g. `abs(u - 0.5)` for a horizontal falloff).
- **Params**: any name you declare — the tweakable knobs.
- **Numbers**: `0.6`, `9`, `-1.5`. **Chars**: `'@'`. **Strings**: `" .:-=+*#%@"` (glyph sets).

**Operators & builtins**

- **Ops**: `+ - * /`, `< <= > >= == !=`, `&& || !`, unary `-`, ternary `c ? a : b`.
- **Builtins**: `abs floor trunc`, `min max`, `clamp select`; curve helpers `mix(a, b, t)`,
  `remap(x, inLo, inHi, outLo, outHi)`, `smoothstep(e0, e1, x)`; and the glyph pair —
  `ramp(v, "chars")` maps `v ∈ [0,1]` across the ramp; `glyph(i, "chars")` indexes it.
- **`let`**: `let name = expr; body` names a reusable subexpression — for readability in bigger
  Facets (it adds no new power; it lowers to the same ops).

```
// strong edges get a directional stroke; everything else, a density ramp
grad_mag > threshold
  ? glyph(clamp(grad_dir * 1.27 + 2.0, 0, 3), "-/|\\")
  : ramp(luma, " .:-=+*#%@")
```

A compile error carries a **byte offset** (`pos`) into the source — use it to place an error
marker at the exact character.

---

## 12 · Registry lifecycle

A small, strict state machine. An author publishes; a moderator decides. There is no other
transition.

```
Publish (author)  →  certified  →  published   (public · renderable by id)
                     (awaiting      or
                      review)       rejected
```

- **Visibility.** The public sees only `published` Facets. A not-yet-published one is visible only
  to its author or a moderator — to everyone else it's a `404` (its existence isn't revealed).
  Reflect this: don't show a "pending" Facet publicly, and expect 404s, not 403s.
- **Render by id** works only for `published` Facets. While a user edits their own draft, preview
  it locally — you already have the bytes.
- **Roles.** `author` may publish; `moderator` may review and decide. Gate the UI accordingly; the
  server enforces it regardless.

---

## 13 · Design & UX notes

- **The output is a grid.** Render glyph text in a monospace face with `line-height: 1` and no
  letter-spacing, or better, paint to a canvas so colour and export are trivial. Respect
  `cellAspect` when sizing.
- **Make colour first-class.** Half-block and tint are the memorable output — give them a real
  canvas, a download-as-PNG, and a copy-as-text for the monochrome case.
- **Preview should feel instant.** It's all local; debounce the editor lightly, keep the
  interpreter compiled once, and re-run on param changes without recompiling.
- **Turn error codes into help.** The gate's rejection `code` is stable and specific —
  `bad_feature_slot`, `program_stride_mismatch`, `unknown_engine`. Map them to plain-language
  editor hints, not raw slugs.
- **Show the trust.** A "verified" badge that actually ran `verifyProgramCertificate` locally is
  more than decoration — it's the platform's promise, made visible.
- **Name things by what they are.** Users make a *style*, not a "program-kind artifact." Reserve
  the internal words for internal code.

---

## 14 · Guarantees to keep

Three invariants the backend is built to hold. The UI should never work around them:

- **`preview == render`.** Always render through the SDK, never a hand-rolled shortcut. If they
  diverge, stop and report it.
- **The server is authoritative.** For anything a user shares or exports, define it by
  `/v1/render` (or a published Facet's certificate). Local preview is a fast mirror, not the
  source of truth.
- **Registry Facets are untrusted.** Run downloaded Facets through the sandboxed (Worker) SDK
  paths, and honour visibility (published-only in public views).

Everything else — layout, motion, palette, flow — is yours. The backend gives you a fast, safe,
provably-consistent engine; the experience on top of it is the part only you can build.

---

*Mosaic · substrate **Mosaic** · engines **Tessera** · styles **Facet***
