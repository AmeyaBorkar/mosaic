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

### D3 — Facet authoring: bootstrap now, custom DSL later *(settled)*
Because the WASM substrate is permanent, the author-facing language is a
swappable compiler frontend. We **bootstrap** Facets in AssemblyScript
(TS-like → WASM) to validate the Tessera contract against real methods, then
design the purpose-built **Facet DSL** from that evidence. Rationale: the right
language cannot be designed before the contract it describes is proven; freezing
it early would bake in the wrong abstraction.

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
density/edge Facets keep the stride-3 L0+L1 path). Proven native≡sandboxed over 64
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
implements by which entry point it exports; the host requires exactly one of
`run`/`run2d` to be present.

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

## Open decisions (from the vision — deliberately not yet frozen)

- *O1 (neighbor visibility), O2 (ASCII vocabulary), O4 (composition), O4.1 (declarative
  composition), and O5 (contract universality) are settled — see D5, D6, D11, D12, D13.*
- **O3 — Facet DSL syntax & semantics.** Deferred by D3 until the contract holds. Two
  engines now share it unchanged (D11), so a DSL is better-informed — the last open O.

## Repository layout

```
crates/
  mosaic-core/     # engine contract, feature vocab, manifest, text composition (slot 5) + composition algebra (O4)
  glyph-atlas/     # shared no_std L2 glyph atlas + SSD matcher (engine + Facet, no drift)
  dither/          # shared no_std Floyd-Steinberg error-diffusion (engine + Facet, no drift)
  mosaic-runtime/  # WASM host: pure, fuel-metered, memory-bounded Facet sandbox
  tessera-ascii/   # first engine: images (L0/L1 density+edges, L2 structural glyph-match)
  tessera-spectral/# second engine: audio PCM -> spectrogram art (proves contract universality, O5)
  mosaic-wasm/     # wasm-bindgen browser bindings: extract + compose + Canvas (built)
  mosaic-compose/  # declarative, JSON-serializable Compositions (O4.1) rendered via a resolver
apps/
  web/             # Next.js shell: editor, controls, live preview, registry   (planned)
facets/ramp/       # bootstrap Facet (Rust -> wasm): density ramp + edge glyphs
facets/spin/       # adversarial Facet: run() never returns (browser-timeout test)
facets/liar/       # adversarial Facet: alloc() returns a wild pointer (bounds test)
facets/structural/ # L2 Facet: sub-cell patch -> nearest atlas glyph
facets/dither/     # propagation Facet: 1-bit error-diffusion dithering (run2d)
packages/
  facet-abi/       # browser Facet host: mirrors run_map, timeout-Worker sandbox
docs/              # this document and future design notes
```

## Undecided housekeeping

- **License.** Not yet chosen; matters for a platform hosting community-authored
  content. Tracked as an explicit open item before first publish.
