# Authoring + controls + colour core (2026-07) — the UI's core functionality

Living design + progress record for the second build: everything a (separately-built) UI
needs beyond the run/registry backend, which is already done. Roadmap point 4 is the UI
itself (external); this makes the *core* it stands on complete. Kept on disk to survive
compaction.

## Why (verified gaps)

The run + preview + registry path is complete. What a UI still needs from the core:

1. **Browser DSL compilation.** `mosaic-dsl` is native-only; the browser can *run* bytecode
   (`facet-abi`) but not *compile* DSL → bytecode. Verified: `mosaic-dsl` compiles to wasm32
   (only dep is `mosaic-vm`, also wasm-ready), so the browser can compile locally — provably
   identical to the server (same crate). **Keystone for an in-browser editor.**
2. **Controls loop.** Manifest types exist (`mosaic_core::manifest`) but nothing surfaces a
   Facet's params to a client, and params are baked into the bytecode. Verified layout: the
   program header is `[0..4] magic · [4..6] stride · [6..8] n_params · [8..10] n_tables ·
   [10..10+n_params*4] params (f32 LE)`, so **a param value can be patched in place** (no
   recompile) and the program still validates.
3. **A one-call render** so the UI doesn't hand-glue `mosaic-wasm` + `facet-abi`.
4. **Colour output.** Everything is monochrome glyphs. Colour = the engine attaching each
   cell's source colour to the Facet's chosen glyph (glyph from the Facet, colour from the
   image) — no Facet-ABI change, and `preview == render` still holds (colour is deterministic
   from the source). A coloured render returns a `{char, rgb}` grid the UI paints.
5. **Publish DSL Facets.** The registry stores wasm modules; a DSL Facet is the shared
   `interp.wasm` + a program. Publishing one is a program-kind submission (validate + certify
   the program, store program + manifest + engine).

## Non-negotiables (unchanged)

Clean bisectable commits; no co-author/collaborative trailers; CI is production (`--locked`,
golden gates, whole repo green); refuse false trade-offs; truth-telling; robust over fast.
`preview == render` and the sandbox guarantees are invariants — never forked or weakened.

## Phase A — the browser authoring + controls loop

- **A1** `mosaic-wasm`: `compile_facet(engine, src, paramsJson) -> { program, manifestJson }`
  — a wasm binding of `mosaic-dsl`. `engine` selects the feature vocabulary: `ascii`
  (stride 3: luma, grad_mag, grad_dir), `spectral` (stride 1: band_energy). Manifest = the
  params (name, value, index) + engine + stride. Compile errors throw with message + position.
  Golden: browser-compiled bytecode == native `mosaic_dsl::compile` (same crate → identical).
- **A2** `patch_params`: overwrite the params section by index. Native (`mosaic-vm`) +
  browser (`facet-abi`). A param change re-renders without recompiling; the patched program
  still validates. Golden: native and browser patch identically; patched run == recompiled run.
- **A3** browser SDK glue (`packages/mosaic-preview`, TS): `compile`, `applyParams`,
  `renderImage(rgba,w,h, program|module, opts) -> {cols, rows, cells}`, wrapping
  `mosaic-wasm` (extract/compose) + `facet-abi` (run). The single dependency a UI imports.

## Phase B — colour output

- **B1** `tessera-ascii` (+ `tessera-spectral`): a per-cell mean-RGB extractor alongside the
  feature buffer, and a colour-aware composition producing a `{codepoint, rgba}` grid (glyph
  from the Facet, colour from the source). No Facet-ABI change.
- **B2** browser: expose the colour extractor via `mosaic-wasm`; the SDK's `renderImage`
  returns per-cell colour so the UI paints coloured glyphs (the "coloured pixel-art" ask).
- **B3** `/v1/render` gains a `color: true` option returning the `{char, rgb}` grid.
  Golden-checked native↔browser like every other render path.

## Phase C — publish DSL Facets (registry program-kind)

- **C1** `mosaic-certify`: certify a *program* — validate via `mosaic_vm`, run through the
  shipped interp over probes, emit a certificate (program-kind). 
- **C2** `mosaic-registry` + `mosaic-server`: a program submission (program + manifest +
  engine) stored and served; `POST /v1/facets` accepts it; render resolves a registry DSL
  Facet by id (interp + program).

## Commit plan (bisectable; each green)

A1 compile binding · A2 patch_params (native+browser+golden) · A3 SDK package.
B1 colour engine · B2 browser colour · B3 server colour option.
C1 certify program · C2 registry+server program-kind. Docs sweep at the end.

## Progress log

- **A1 done** — `mosaic-wasm` `compileFacet(engine, src, paramsJson) -> CompiledFacet`
  (`program` + `manifestJson`). Wraps `mosaic-dsl`; `ascii`/`spectral` vocab; native-testable
  `compile_program` core. 3 unit tests incl. **byte-identical to native compile**. Builds to
  wasm32; fmt/clippy/test green.
- **A2 done** — `mosaic_vm::patch_params(program, values)` (in-place, no_std) + browser
  `facet-abi applyParams(program, values)` mirror: overwrite the params section (LE f32 at
  offset 10+i*4) without recompiling; patched program still validates. Native test + 3 TS
  tests (offset correctness + a patched threshold changing the render through the real
  interp). No wasm drift (DCE'd from interp). Registered in package.json + CI.
- (next) A3 — browser SDK glue (`packages/mosaic-preview`).
