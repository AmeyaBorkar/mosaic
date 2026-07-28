# Mosaic — Architecture & Decisions

This document records the technical architecture and the decisions made so far,
with their rationale. It complements `vision.md` (the *what* and *why*) with the
*how*. Naming: **Mosaic** (platform), **Tessera** (engine, one per domain),
**Facet** (style, many per engine).

## Layered architecture

1. **Mosaic (platform)** — the domain-agnostic substrate, built once: the Facet
   registry, the safe runtime that executes Facets, auto-generated controls +
   live preview, and composition.
2. **Tessera (engine)** — one per domain. Defines what the media is and how it
   decomposes, by filling the five-slot **engine contract**:

   | Slot | Question it answers |
   |------|---------------------|
   | Input | What media does the engine take? |
   | Unit | How is the media decomposed into workable pieces? |
   | Feature vocabulary | What may a unit measure about itself? |
   | Output primitive | What does a single unit become? |
   | Composition | How are the output pieces reassembled into a whole? |
3. **Facet (style)** — the community layer, many per Tessera. Declared
   parameters (the user-facing knobs, which generate the controls) + pure logic
   (the encoded method).

## Decisions

### D1 — Facet execution substrate: WebAssembly *(settled)*
Facets are untrusted, community-authored code that must be **pure**
(no network/disk/clock), **deterministic** (same Facet + same media = same
render), **metered** (bounded CPU/memory), **fast** (live preview over millions
of units), and **identical** client- and server-side. WASM provides all of it by
construction:
- *Purity by default* — a module has zero ambient authority; we grant no imports.
- *Determinism* — core WASM arithmetic is bit-identical across engines (NaN
  payloads canonicalized; no threads / relaxed-SIMD).
- *Metering* — deterministic fuel plus memory caps.
- *One runtime, both sides* — native WASM in the browser; Wasmtime on the server.
- *Pluggable authoring* — WASM is an ABI, not a language: a Facet DSL for most
  authors, bring-your-own-compiled for power users, all sandboxed identically.

A bespoke bytecode VM would reinvent this, worse. WASM is the floor.

### D2 — Core language Rust, shell TypeScript *(settled)*
Each Tessera engine is written **once in Rust** and compiled to WASM, so the
*same* engine code powers browser live-preview and server batch render with zero
drift. Rust also hosts the runtime, metering, and the future Facet compiler.
**TypeScript** (Next.js + pnpm) owns the editor, the auto-generated controls,
live preview, and the registry.

### D3 — Facet authoring: bootstrap now, custom DSL later *(settled — realized by D14)*
Because the WASM substrate is permanent, the author-facing language is a
swappable compiler frontend. We **bootstrapped** Facets as hand-written `no_std`
Rust → WASM (`facets/ramp`, `dither`, `structural`) to validate the Tessera
contract against real methods, then designed the purpose-built **Facet DSL** from
that evidence — now shipped as **D14** (`mosaic-dsl` → validated bytecode, run by one
interpreter Facet). Rationale: the right language cannot be designed before the
contract it describes is proven; freezing it early would bake in the wrong
abstraction. (An earlier draft named AssemblyScript for the bootstrap; `no_std` Rust
was used instead, keeping one toolchain and the same determinism discipline.)

### D4 — Windows toolchain: rustup `stable-msvc` + VS Build Tools *(settled)*
MSVC host toolchain — the most compatible on Windows. WASM builds need no native
linker (Rust uses `rust-lld`); only native/server builds and `cargo test`
require the MSVC C++ tools.

### D5 — Unit access model: neighborhood gather + opt-in propagation *(settled — O1)*
A unit is a pure function of features gathered over a bounded read-only
neighborhood (radius R, declared; R=0 = self-only). This keeps the common path
fully parallel and deterministic while covering all read-context methods (edges,
gradients, contours, structure) — "read a neighborhood" (gather) has *no*
write-dependencies, so `output[i] = f(readonly_input[neighborhood(i)])` stays
embarrassingly parallel.

The one genuinely-sequential pattern — feedback/propagation (e.g. error-diffusion
dithering) — is confined to a separate, **opt-in** capability: a Facet returns a
*residual* alongside its output, and the engine diffuses it to not-yet-processed
units along a declared kernel and traversal order. The Facet stays pure; the
engine owns the ordering, so it stays deterministic. This isolates the only real
cost instead of imposing it on every Facet.

**Now implemented.** The first propagation method — 1-bit Floyd–Steinberg
error-diffusion dithering — ships via a dedicated 2-D Facet ABI (D10): the engine
hands the Facet the grid shape and the Facet runs the sequential feedback loop inside
the sandbox, deterministically. The kernel lives in one shared `no_std` crate
(`crates/dither`) compiled into both the native engine (`render_dither`) and the wasm
Facet (`facets/dither`), so the sequential path is bit-identical native and wasm —
proven by a 64-random-image sandboxed≡native sweep and the browser≡native golden. (A
region of flat grey stipples into a mix of glyphs — impossible with pure gather.)

### D6 — First feature vocabulary (ASCII): L0 + L1 + L2 *(settled — O2)*
The ASCII Tessera's vocabulary ceiling is:
- **L0 — Luminance** (cell mean, min/max/variance): density ramps.
- **L1 — Gradient** (magnitude + orientation, via a neighborhood structure
  tensor): edge-aware directional glyphs. Depends on D5 gather.
- **L2 — Sub-cell structure** (an N×M luminance patch + glyph-atlas access): the
  Facet shape-matches its patch against candidate glyphs however it likes.

This is the vision's full "brightness → edges → sub-pixels" progression.
Implementation may be staged (L0+L1 first, L2 immediately after); the contract
reserves all three so nothing caps later. Color is deliberately excluded from
ASCII and added by the ANSI Tessera as an extension of the same vocabulary shape.

In `mosaic-core`, a vocabulary is a `feature::FeatureSchema` — an ordered list of
typed fields (`Scalar` / `Vector` / `Patch`) each tagged with its `Gather`
radius. The concrete ASCII fields live in the ASCII engine; the schema is the
generic ABI the runtime uses to marshal features to the (WASM) Facet.

**L2 now implemented.** The engine extracts an 8×8 sub-cell luminance patch
(`extract_structural`, a self-only `Patch{8,8}`, stride 64) and a Facet matches it
to the closest glyph by sum-of-squared-differences; density *and* structure fall
out of that one nearest-glyph rule. The atlas + matcher live in a single `no_std`
`glyph-atlas` crate compiled into **both** the native engine and the untrusted wasm
Facet (`facets/structural`) — one matcher, not two that could drift. L2 is opt-in
(only structural Facets pay the 64-slot stride, via a separate `extract_structural`;
density/edge Facets keep the stride-8 L0+L1+position+colour path). Proven native≡sandboxed over 64
random images and browser≡native end-to-end.

### D7 — Facet runtime: wasmtime, pinned current *(settled)*
`mosaic-runtime` executes Facets on wasmtime with fuel metering enabled, a
per-execution memory cap (`StoreLimits`), and **zero imports** — so purity is
structural, not policed. Verified end-to-end: a module declaring any import fails
to instantiate; an infinite loop is halted by fuel; repeated runs are identical.

**Full sandbox hardening (after an independent adversarial audit).** `StoreLimits`
caps not only linear-memory size but table elements and memory/table/instance
*counts*, and the engine `Config` disables threads (hence shared memory),
multi-memory, and relaxed-SIMD while enabling NaN canonicalization. This closes
host-OOM vectors the linear-memory cap alone missed (oversized `funcref` tables,
many memories, shared memory) and makes execution deterministic across platforms.
Compilation is size-bounded, and the map ABI rejects zero stride and bounds every
untrusted size before allocating. Each vector has an adversarial test. The engine
uses `libm` for transcendentals (e.g. `atan2f`) so gradient orientation is
bit-identical across platforms and across the native/wasm builds.

Pin wasmtime to the **latest** release, not an older "safe" version. An old
wasmtime (v27) under a current rustc (1.97) aborted on trap delivery on Windows
(`STATUS_STACK_BUFFER_OVERRUN`, non-unwinding panic) — a low-level unwinding
mismatch between the compiler and a stale runtime, not a code bug and not fixable
via `Config`. A toolchain-contemporary wasmtime (v47) fixed it.

### D8 — Facet ABI: feature buffer in, `u32` tokens out *(settled)*
A Facet exports `memory`, `alloc(i32) -> i32`, and
`run(in_ptr, out_ptr, ncells, stride)`. The host (`mosaic-runtime::Sandbox::run_map`)
allocates through the guest's *own* allocator, writes the per-cell feature buffer
as little-endian `f32`, calls `run`, and reads back one `u32` output token per
cell (for ASCII, a glyph codepoint). The call is **batch** (whole buffer), not
per-cell, so a render is a single boundary crossing rather than millions.

`alloc`/`run` are guest exports, so purity holds; every crossing is bounds-checked
by wasmtime, so a malformed Facet errors rather than corrupting the host; and
untrusted output codepoints are validated (`char::from_u32`, `U+FFFD` fallback)
before composition. Proven end-to-end: `facets/ramp` — a real `no_std` Rust → wasm
Facet (499 bytes) — renders images to ASCII inside the sandbox with byte-identical
output to the native engine path (hermetic test).

### D9 — Browser Facet execution: native WebAssembly in a timeout-metered Worker *(settled)*
The server render (D7) runs Facets on wasmtime with fuel + `StoreLimits`. The
browser has no wasmtime, and core WebAssembly there has **no fuel**. Rather than
ship a second, weaker sandbox, we split by trust role:

- **Server = authority.** wasmtime, fuel-metered, deterministic — the render of
  record.
- **Browser = liveness/preview.** The Facet runs on the browser's *own*
  `WebAssembly` engine for instant feedback as the author edits controls.

Both sides enforce the *same* guarantees where it matters. **Purity is structural
on both:** instantiate with zero imports, and reject any module that *declares* an
import before it can run (`WebAssembly.Module.imports`), mirroring wasmtime's
import-free instantiation. Both speak the *same* ABI (D8): the browser host
(`packages/facet-abi`) mirrors `run_map` exactly — identical stride/length/overflow
checks, the same alloc-through-the-guest marshalling, little-endian `f32` in and
`u32` tokens out. A **golden vector emitted by the proven native `run_map`** on the
*real* Facet wasm pins the two implementations byte-for-byte (conformance test), so
"preview == render" is verified, not assumed.

**Metering without fuel.** A synchronous WASM infinite loop cannot be preempted on
the main thread, so untrusted Facets execute inside a **Web Worker** under a
wall-clock **timeout**; a Facet that overruns is `terminate()`d and surfaces a
clean error, and a memory bomb is contained to the worker rather than the page.
The correctness-critical marshaller is a pure synchronous function, isolated from
the worker/timeout policy, so it is tested directly; a real never-returning Facet
fixture proves the timeout actually kills a hang. Determinism holds for
well-behaved Facets because core WASM arithmetic is bit-identical across engines
and our Facets avoid NaN-payload-dependent branches (the engine's transcendentals
already use `libm`, D7).

**Proven end-to-end.** The engine bridge `mosaic-wasm` exposes `extract` and
`compose` (the *same* Rust the server runs) to the browser. Extract-in-wasm is
bit-identical to native `feature::extract` (incl. `libm::atan2f`), and the whole
client pipeline — `extract` → Facet (via `facet-abi`) → `compose` — reproduces the
authoritative native `render_ascii` over a golden image set. Preview is now a
*checked* equal of the render, not a hope.

**Post-audit hardening.** An adversarial audit hardened the untrusted boundary. The
browser now enforces a linear-memory cap the way the native `StoreLimits` does: a
Facet must declare a bounded memory maximum ≤ 16 MiB (`@mosaic/facet-abi` rejects any
that does not, and the bundled Facets are built with `--max-memory`), which the
engine enforces on `memory.grow` — so a memory-bomb Facet is contained, not merely
raced against the timeout. Feature extraction is byte-budgeted (not cell-counted) so
the stride-64 L2 path cannot be driven to a multi-GB allocation; the guest bump
allocators bounds-check; `compose` masks control/bidi codepoints out of untrusted
output; and the browser host checks export arities and i32 ranges to match the native
`run_map`.

**Known browser parity limits.** Three native determinism controls have no
`WebAssembly` API equivalent and so cannot be *enforced* in the browser: relaxed-SIMD
rejection, NaN-payload canonicalization, and a deterministic instruction (fuel)
budget — the browser bounds time by a wall-clock timeout instead. A Facet that uses
relaxed-SIMD, branches on NaN bits, or overruns the timeout can therefore diverge
from (or be rejected by) the authoritative server render. The planned Facet registry
closes this with a submission-time conformance gate (a decode-pass that rejects the
disallowed features + a golden-token sweep against the server); until then the server
render is the record of truth, and the browser preview is exact only for Facets that
avoid these.

### D10 — Propagation ABI: `run2d` for feedback methods *(settled)*
Gather Facets export `run(in_ptr, out_ptr, ncells, stride)` (D8) and see no grid
geometry — right for the embarrassingly-parallel path. Feedback methods (error
diffusion) need neighbour positions, so a propagation Facet instead exports
`run2d(in_ptr, out_ptr, cols, rows, stride)` and is handed the 2-D shape;
`mosaic-runtime::run_map_2d` and `@mosaic/facet-abi::runFacetMap2d` invoke it. This is
**additive** — gather Facets are unchanged — and both hosts share the *same*
marshalling as the gather ABI (bounds/overflow checks, zero imports, memory cap), so
the propagation path inherits every sandbox guarantee. A Facet declares which ABI it
implements by which entry point it exports; each host looks up the entry point the
caller asks for (`run` for the gather ABI, `run2d` for propagation, and — with D14 —
`run` plus `load_program` for the DSL interpreter) at call time. There is no admission
check that *exactly one* is present; a caller selects the ABI by which `run_map` /
`run_map_2d` / `run_program` entry point it invokes.

### D11 — Second engine `tessera-spectral`; composition is a substrate primitive *(settled — O5)*
The platform's load-bearing claim is that the five-slot contract (D5/D6) is
*universal*, not shaped around images. `tessera-spectral` (audio PCM → spectrogram text
art) tests it with a different Input (a 1-D signal, not RGBA) and a different feature
vocabulary (per-band spectral energy via a Hann-windowed Goertzel filterbank, not
luminance), while filling the same five slots.

The proof is a passing test, not an assertion: the *existing image Facets* —
`facet-ramp` (gather) and `facet-dither` (propagation), the exact WASM binaries,
byte-identical by SHA-256 — run **unmodified** in the sandbox over spectral features and
produce output byte-identical to the native spectral references, across 32 random
signals spanning sample rates and grid shapes. A Facet is confirmed to be a
domain-agnostic `feature-vector → token` function: it reads slot 0 and cannot tell image
luminance from audio band energy.

Building the second engine forced the correct layering. Text-grid composition
(`compose_codepoints` + untrusted-glyph masking) is domain-agnostic, so it moved out of
`tessera-ascii` into `mosaic-core::compose` (Mosaic slot 5). Both engines now share one
composition implementation and one untrusted-text boundary — the crate graph enforces
the layering instead of convention. Determinism uses the D6 discipline (libm for every
transcendental, no `mul_add`), so the STFT is bit-reproducible — and
`mosaic-wasm::extract_spectral_features` plus a native↔wasm golden
(`crates/mosaic-wasm/test/spectral.test.ts`) now prove it: the browser path is
bit-identical to native, giving this engine the same preview == render guarantee as the
ASCII engine, end to end.

### D12 — Composition algebra: a painter's-algorithm Canvas *(settled — O4)*
Composition — combining whole renders into one artifact — is a Mosaic-substrate concern,
"built once", not an engine feature. `mosaic-core::composite` is the primitive: a `Canvas`
built up by `place(layer, row_off, col_off, blend)` calls (painter's algorithm). One
primitive unifies **overlay** (place at the origin), **layout / tiling** (place at an
offset, clipping), and **masking** (per-cell `Layer` coverage). A glyph cell has no true
alpha, so partial coverage resolves through an ordered Bayer dither
(`Blend::StippleOver`) — perceptual blending of discrete glyphs with no impossible
half-glyph; `Over`/`Under`/`Replace` cover crisp compositing.

It is domain-agnostic (operates only on `u32` output tokens, so an image render and an
audio render composite identically) and safe: `Canvas::into_text` routes every surviving
cell through `compose_codepoints`, so a composed artifact inherits the untrusted-glyph
boundary and runs no untrusted code — the Facets already executed in the sandbox; this is
pure host-side grid math on their outputs. Deterministic (the Bayer matrix is constant; no
transcendentals), so the browser `Canvas` binding reproduces native composition
byte-for-byte (`composite.test.ts`) — proven on a genuine cross-engine artifact (an image
ASCII render stacked with an audio spectrogram) plus energy-driven `StippleOver`.

This primitive is the foundation the declarative, shareable Composition (D13) wraps.

### D13 — Declarative Composition: a shareable, serialized layer stack *(settled — O4.1)*
`mosaic-compose` is the declarative layer above the D12 primitive: a `Composition` is pure,
JSON-serializable data — a canvas plus an ordered stack of layers, each naming the engine +
Facet + input that produces it, its placement, blend, and coverage mode. It is a
first-class shareable artifact, exactly like a Facet: the registry stores it, the web shell
renders it.

The schema carries everything *except how to run an engine* — that is the host's job.
`render()` takes a `LayerResolver` (the seam the registry/server fills): given a layer's
`LayerSource`, the resolver produces its token grid (e.g. by running a Facet in the
sandbox), and `render` composites the stack through the O4 `Canvas`. So the crate stays
engine-agnostic (it depends only on the substrate), and rendering inherits every O4
guarantee — no untrusted code in the compositor, and the final text passes the
untrusted-glyph boundary. Proven end-to-end: a JSON composition drives the real image and
audio engines to one artifact, byte-stable across a serialize → parse → render round-trip.

### D14 — Glint, the Facet authoring language: bytecode + one interpreter Facet *(settled — O3)*
Authoring a Facet no longer requires `no_std` Rust. **Glint** (the crate is `mosaic-dsl`)
compiles a small expression language — a per-cell expression over named features and params
producing one glyph — to a compact bytecode; a single interpreter Facet (`facets/interp`,
wrapping the shared
`mosaic-vm`) runs *any* bytecode in the existing sandbox. This was chosen over a wasm-codegen
backend (a large build emitting opaque output) and over host-side AST interpretation (which
would widen the attack surface): the bytecode interpreter reuses the already-audited sandbox
unchanged, needs no compiler backend, and the shareable-bytecode model mirrors D13.

The layering separates the ISA from the surface. `mosaic-vm` is the contract — a validated,
`no_std`, forbid-unsafe stack VM: every feature/param/table index is bounds-checked, the
stack effect is statically simulated, and v1 is straight-line so termination is guaranteed;
only `f32` arithmetic and libm `floorf`/`truncf` (no transcendentals, no `mul_add`) so it is
bit-identical native vs wasm. `mosaic-dsl` is one frontend; a future visual/node editor
could target the same bytecode. The VM is domain-agnostic — glyph ramps and edge sets are
codepoint tables in the *program*, not baked in.

Untrusted bytecode is validated twice (host-side by the compiler's self-check, and again
inside the sandbox before execution). `mosaic-runtime::run_program` loads it via the Facet's
`load_program` export, then runs the gather ABI with the same bounds-checks, zero imports,
and metering as any Facet. Proven end-to-end: an author's DSL text compiles and runs
untrusted in the sandbox byte-identical to the native reference — `ramp(luma, …)` matches
native density exactly, and a branchy density-or-edge Facet is deterministic and
non-degenerate. v1 targets the gather ABI; propagation (`run2d`) is an additive follow-on.

### D15 — Server-authoritative conformance gate and render *(settled)*
The browser enforces the conformance profile "user side" (D9); the **server is the
authority**. `mosaic-certify` is a single gate with two layers: a static `check_profile`
that admits *exactly* what the browser mirror admits (zero imports; one bounded, non-shared,
32-bit linear memory within the page cap; at most one 32-bit table within the element cap;
the ABI exports; one map entry point), and `certify`, which runs the admitted Facet through
the proven native host over a deterministic probe suite and emits a **Certificate** — golden
`(features → tokens)` vectors bound to the exact bytes by a SHA-256. The browser's
`verifyCertificate` (in `@mosaic/facet-abi`) replays those probes and must reproduce every
outcome, so `preview == render` becomes a checked property for *any* certified Facet, not
only the shipped goldens. The probe suite is an honest representative *sample*, not a proof
over all inputs.

The authoritative render is `mosaic-server`'s `POST /v1/render`. It mirrors the browser
bridge's pipeline — `tessera_*::feature::extract*` → `Sandbox::run_map`/`run_map_2d` →
`mosaic_core::compose_codepoints` — run natively, using the *same source* the browser
compiles to wasm, so the server render is bit-identical to the preview by construction. The
engine name selects the feature vocabulary (and stride); the certified ABI kind selects
gather vs propagation. Input is authoritative raw bytes (RGBA8 / f32 PCM) — no decode
ambiguity. CPU-bound work runs on blocking workers off the async executor.

### D16 — Facet registry with bearer auth and moderation *(settled)*
`mosaic-registry` is the store — a `Store` trait (insert / get / get_bytes / list / set_state)
with a durable `RedbStore` (pure-Rust, embedded, ACID; no C toolchain, no server process) and
an `InMemoryStore` the endpoints and auth are tested against. The lifecycle is a small,
explicit state machine: an author **publishes** (`POST /v1/facets`), which runs the gate and
stores the Facet `Certified` awaiting review; a **moderator** transitions it `Certified →
Published | Rejected` (`POST /v1/facets/{id}/moderate`), the only transitions permitted (any
other is a 409). Public listing shows `Published` only; a moderator reviews the queue via
`?state=certified`. A not-yet-published Facet is visible only to its author or a moderator,
and to everyone else is a 404 — the registry never reveals that an unpublished Facet exists.

Auth is `Authorization: Bearer <token>` resolving to a `Principal` (id + roles: author,
moderator). Tokens are configured out of band (`MOSAIC_TOKENS`, a JSON file kept out of the
repo) and stored **hashed** — the table is `SHA-256(token) → Principal`, so no plaintext
token sits in memory and a lookup is constant-time in the token value on a preimage-resistant
digest (the standard opaque-API-token pattern). The service is container-ready (a multi-stage
`Dockerfile`, non-root slim runtime); the durable registry path is `MOSAIC_DB`. CD (image
push / deploy) needs registry credentials and is left an explicit, unwired hook rather than
faked.

### D17 — Publish and run authored DSL Facets (registry program-kind) *(settled)*
A DSL Facet is authored and previewed entirely in the browser (D14, D9), but was a local
dead-end: the registry stored only self-contained wasm modules. D17 closes the author → share
seam. A stored Facet is now a tagged **`FacetArtifact`**: a `Wasm` module (as before) or a
`Program` — `mosaic-vm` bytecode that runs on the **one shared interpreter Facet** (`facet-interp`),
authored for a named `engine` (which fixes its stride). Only the bytecode is untrusted; the
interpreter is a trusted, first-party module (shipped as a committed asset and re-validating
every program it runs).

Certification mirrors the wasm path (D15) for bytecode: `certify_program` validates natively
(`mosaic_vm::validate`, for a precise rejection code and the declared stride), then probes the
program through the interpreter in the authoritative sandbox (`run_program`) and emits a
`ProgramCertificate` — the golden `features → tokens` bound to the exact bytecode by SHA-256.
Native `mosaic_vm` and the sandboxed interpreter are byte-identical by construction, so this is
the same `preview == render` guarantee (D9), now for authored DSL. The browser closes the loop:
`verifyProgramCertificate` replays the probes through its own interpreter and must reproduce
every outcome — a *checked* property, not a hope, for any published program.

`POST /v1/facets` accepts a wasm module (`wasm`) or a program (`program` + `engine`); a program
is refused for an unknown engine or a stride that disagrees with the engine it targets. Render
resolves a Facet by registry **id** (only `Published` renders by id) as well as inline, running
a program on the shared interpreter at its declared stride — the stride check fully guards
vocabulary compatibility, so the render path needs no engine table. Bytes are served
kind-specifically: `/wasm` for a module, `/program` for bytecode.

### D18 — Cell position in the ASCII vocabulary (`u`/`v`) *(settled — capability roadmap item 1)*
The first "see more, not compute more" unlock: the `ascii` engine now measures each cell's
**normalized centre position** as a self-only `Vector{2}` `position` — `u = (col+0.5)/cols`,
`v = (row+0.5)/rows`, each in `(0, 1)` — appended to the core vocabulary (stride **3 → 5**,
slots 3–4). Glint reads them by the names `u`/`v`; there are **no new VM ops**, because position
is just more numbers the trusted engine produces. It unlocks gradients, vignettes, spotlights and
spatial masks, and is the prerequisite for spatially-varying noise and coordinate fractals
(roadmap 5–6).

Cell-centre (not corner) makes it symmetric at both edges and free of any divide-by-zero on a
degenerate 1×N grid. The cast, the `+ 0.5`, and the divide are each exact or correctly-rounded in
`f32`, so it is bit-identical native vs wasm and `preview == render` (D9) carries over unchanged —
proven by the reblessed render/DSL/program-cert goldens, whose browser replays now exercise a
position-shaded Facet. One coupled invariant moved with it: the wasm **gather-certificate probe
range** widened from 1..=4 to 1..=6 so the stride-5 `ascii` vocabulary stays inside the per-Facet
certified envelope. (The stride-64 `ascii-structural` path sits outside that range by design — its
browser≡native parity is proven end-to-end by the render golden's `structuralText`, one matcher
compiled both ways, not by these per-Facet probes.) Raw integer `cx`/`cy` remain a trivial
additive extension (stride 5 → 7) if absolute, resolution-dependent periodicity is ever wanted.

### D19 — Cell colour in the ASCII vocabulary (`r`/`g`/`b`) *(settled — capability roadmap item 2, colour family)*
The second "see more" unlock, and the first slice of the "richer engine features" axis: the
`ascii` engine now measures each cell's **mean colour** as a self-only `Vector{3}` `color` —
`r`, `g`, `b`, each normalized to `[0, 1]` — appended after position (stride **5 → 8**, slots
5–7). Glint reads them as `r`/`g`/`b`; again **no new VM ops**. It unlocks colour-aware styling —
duotone by hue, warm/cool glyph sets (`r - b`), chroma-driven density — the input side that
complements the colour *output* modes (half-block, tint).

Crucially it is **not** a second colour path: the feature reuses the exact deterministic integer
mean (`color::mean_color`) that the tint and half-block renders already use, so a Facet reads
*precisely* the colour it would be tinted with — one source of truth, asserted in
`color_slots_are_the_normalized_cell_mean`. Each channel is an exact 0..255 integer and `/255.0`
is correctly rounded, so the extractor stays bit-identical native vs wasm and `preview == render`
(D9) holds — proven by the reblessed render/DSL/program-cert goldens, whose browser replays now
exercise a colour-shaded Facet. As in D18, the coupled invariant moved too: the gather-certificate
probe range widened `1..=6 → 1..=8` so the stride-8 `ascii` vocabulary stays inside the per-Facet
certified envelope. Neighbour statistics and multi-scale luma remain the next families on this axis.

### D20 — Sub-cell braille render mode *(settled — capability roadmap item 3)*
The first "resolve finer" unlock: a `braille` render mode that packs a 2×4 grid of Unicode braille
dots (`U+2800`–`U+28FF`) into every character cell, for ~8× the spatial resolution of the glyph
render — dense monochrome photos and crisp line art. Like the colour render modes (`halfblock`,
tint), it is a **no-Facet** engine render: `render_braille` thresholds each sub-cell's mean
luminance (a dot is raised where the sub-cell is bright, `≥ 0.5`) and packs the eight dots into the
codepoint. It touches **neither the DSL, the stride, certificates, nor the gather-probe range** — a
purely additive output path, wired everywhere `halfblock` is (server `RenderRequest::Braille`,
`mosaic_wasm::renderBraille`).

Threshold → bitmask → codepoint is exact integer logic over deterministic `f32` sub-cell means, so
the browser render is bit-identical to the server's — `preview == render` (D9), proven by a new
`brailleText` column in the render golden that the browser replays via `renderBraille`. Braille
glyphs are printable and pass `compose_codepoints`' unsafe-glyph mask untouched. A fixed 0.5
brightness threshold keeps v1 parameter-free (matching `halfblock`); an invert flag or an adaptive
threshold is an additive follow-up.

## Open decisions (from the vision — deliberately not yet frozen)

- *All of the vision's open questions — O1, O2, O4, O4.1, O5, and now O3 — are settled;
  see D5, D6 and D11–D14.*
- The platform build-out is now largely engineering-complete: the **conformance gate**
  (server-authoritative certify, D15), the **authoritative server render endpoint** (D15),
  the **registry (+ auth/moderation)** (D16), and **publishing + running authored DSL Facets**
  (D17) are built. What remains is the **web shell** (`apps/web`, the Next.js editor/preview
  over these endpoints) and, on the engine track, new Tesserae (ANSI/colour, halftone,
  data→art). These are engineering, not open decisions.

## Repository layout

```
crates/
  mosaic-core/     # engine contract, feature vocab, manifest, text composition (slot 5) + composition algebra (O4)
  glyph-atlas/     # shared no_std L2 glyph atlas + SSD matcher (engine + Facet, no drift)
  dither/          # shared no_std Floyd-Steinberg error-diffusion (engine + Facet, no drift)
  mosaic-runtime/  # WASM host: pure, fuel-metered, memory-bounded Facet sandbox
  mosaic-certify/  # server-authoritative gate: static profile + golden-probe certificate (D15)
  tessera-ascii/   # first engine: images (L0/L1 density+edges, L2 structural glyph-match)
  tessera-spectral/# second engine: audio PCM -> spectrogram art (proves contract universality, O5)
  mosaic-wasm/     # wasm-bindgen browser bindings: extract + compose + Canvas + verifyCertificate (built)
  mosaic-compose/  # declarative, JSON-serializable Compositions (O4.1) rendered via a resolver
  mosaic-vm/       # DSL bytecode VM (no_std): validated, deterministic per-cell interpreter (O3)
  mosaic-dsl/      # the Facet DSL compiler: expression text -> bytecode (O3)
  mosaic-registry/ # Facet registry: Store trait + durable redb backend + in-memory (D16)
  mosaic-server/   # authoritative HTTP service: certify, render, registry, auth/moderation (D15/D16)
Dockerfile         # multi-stage production image for mosaic-server (non-root slim runtime)
apps/
  web/             # Next.js shell: editor, controls, live preview, registry   (planned)
facets/ramp/       # bootstrap Facet (Rust -> wasm): density ramp + edge glyphs
facets/spin/       # adversarial Facet: run() never returns (browser-timeout test)
facets/liar/       # adversarial Facet: alloc() returns a wild pointer (bounds test)
facets/structural/ # L2 Facet: sub-cell patch -> nearest atlas glyph
facets/dither/     # propagation Facet: 1-bit error-diffusion dithering (run2d)
facets/interp/     # DSL interpreter Facet: runs mosaic-vm bytecode in the sandbox (O3)
packages/
  facet-abi/       # browser Facet host: mirrors run_map/run_map_2d/run_program, timeout-Worker sandbox
docs/              # this document and future design notes
```

## Undecided housekeeping

- **License.** Settled: dual MIT OR Apache-2.0 (see `LICENSE-MIT`, `LICENSE-APACHE`,
  and the README contribution clause).
