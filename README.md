# Mosaic

[![CI](https://github.com/AmeyaBorkar/mosaic/actions/workflows/ci.yml/badge.svg)](https://github.com/AmeyaBorkar/mosaic/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

A platform for **community-authored visual methods** — a shared substrate where
authors publish methods for turning media into art (ASCII, ANSI, halftone, …) and
users run those methods on their own media, with a live in-browser preview that is
**provably identical** to the server-side render.

See [`vision.md`](./vision.md) for the vision and
[`docs/architecture.md`](./docs/architecture.md) for the architecture and every
decision made so far (D1–D9).

## The three layers

| Layer | Name | Role |
|-------|------|------|
| Platform | **Mosaic** | Domain-agnostic substrate: the Facet registry, the safe runtime that executes Facets, auto-generated controls, and composition. |
| Engine | **Tessera** | One per domain (ASCII first; ANSI, halftone, data→art to follow). Fills a five-slot contract: Input · Unit · Feature vocabulary · Output primitive · Composition. |
| Style | **Facet** | The community layer, many per Tessera. Declared parameters (which generate the controls) + pure, sandboxed logic (the encoded method). |

The core pattern is **decompose → measure → map → recompose**: an engine breaks
media into units and measures a declared *feature vocabulary* over each; a Facet
maps features to an output primitive; the engine recomposes the whole.

## Why WebAssembly

Facets are untrusted, community-authored code. WASM makes them **pure** (zero
imported capabilities), **deterministic** (bit-identical arithmetic across
machines), **metered** (bounded CPU/memory), and **identical client- and
server-side**. The server executes Facets on [wasmtime](https://wasmtime.dev) with
fuel + memory/table/instance caps; the browser runs the *same* Facet wasm on its
own WebAssembly engine inside a timeout-bounded Web Worker. Both enforce purity
structurally and speak one ABI, and a golden-vector conformance test pins the two
hosts together — so **preview == render** is verified, not hoped.

## Status

The full pipeline is implemented and proven end-to-end, **native and browser**:

- **`mosaic-core`** — the domain-agnostic engine contract (feature schema, Facet
  manifest, access model).
- **`mosaic-runtime`** — the pure, fuel-metered, memory/table/instance-bounded
  wasmtime sandbox that executes untrusted Facets.
- **`glyph-atlas`** — a shared `no_std` glyph atlas + sub-cell matcher, compiled
  into *both* the native engine and the wasm Facet so there is one implementation,
  not two that could drift.
- **`tessera-ascii`** — the first engine (image → ASCII) with the full vocabulary:
  **L0** luminance density, **L1** Sobel gradient edges, **L2** sub-cell structural
  glyph-matching.
- **`mosaic-wasm`** — wasm-bindgen browser bindings (`extract` + `compose`, the
  same Rust the server runs).
- **`packages/facet-abi`** — the browser-side Facet host: mirrors the native ABI
  and sandboxes untrusted Facets in a timeout Worker.
- **Facets** — `ramp` (density + edges), `structural` (L2 glyph-match), and `dither`
  (1-bit Floyd–Steinberg error-diffusion — the propagation/feedback class via the 2-D
  `run2d` ABI), plus `spin`/`liar` adversarial fixtures for the sandbox tests.

**Verification:** 49 Rust tests + 16 JS tests, `clippy -D warnings` clean, with
adversarial sandbox tests and native≡wasm conformance sweeps over random images.

Not yet built (see the architecture doc): the Facet DSL (O3), cross-engine
composition (O4), a second domain (O5), the registry, and the web UI shell.

## Repository layout

```
crates/
  mosaic-core/     # engine contract, feature vocabulary, Facet manifest
  glyph-atlas/     # shared no_std L2 glyph atlas + SSD matcher (engine + Facet)
  mosaic-runtime/  # WASM host: pure, fuel-metered, memory-bounded Facet sandbox
  tessera-ascii/   # the first engine: image → ASCII (L0/L1/L2)
  mosaic-wasm/     # wasm-bindgen browser bindings: extract + compose
facets/            # guest Facets (Rust → wasm): ramp, structural, spin, liar
packages/
  facet-abi/       # browser Facet host (TypeScript): ABI mirror + Worker sandbox
docs/              # architecture and design notes
scripts/           # CI helpers
```

## Getting started

**Prerequisites**

- Rust (stable) with the `wasm32-unknown-unknown` target (pinned in
  [`rust-toolchain.toml`](./rust-toolchain.toml)). On Windows, the MSVC C++ Build
  Tools are needed for native builds and `cargo test`; the wasm path is not.
- [Node](https://nodejs.org) 24+ (the JS tests run on the built-in test runner and
  TypeScript type-stripping — no test framework to install).
- [`wasm-pack`](https://rustwasm.github.io/wasm-pack/) for the browser bridge.

**Rust — build, lint, test**

```sh
cargo test --workspace
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all
```

**Rebuild the guest Facet wasm fixtures** (only when a Facet's source changes):

```sh
# RUSTFLAGS caps each Facet's linear memory at 16 MiB, so the browser enforces the
# same ceiling the native sandbox does (a memory-bomb Facet cannot exceed it).
for f in ramp spin liar structural; do
  RUSTFLAGS="-C link-arg=--max-memory=16777216" \
    cargo build --manifest-path "facets/$f/Cargo.toml" --target wasm32-unknown-unknown --release
done
# then copy the outputs into the fixture locations and refresh the goldens:
cargo run -p mosaic-runtime --example emit_golden
cargo run -p tessera-ascii  --example emit_render_golden
```

**Browser bridge + JS tests**

```sh
cargo install wasm-pack
wasm-pack build crates/mosaic-wasm --target nodejs --dev --out-dir pkg
node --test \
  packages/facet-abi/test/conformance.test.ts \
  packages/facet-abi/test/adversarial.test.ts \
  packages/facet-abi/test/timeout.test.ts \
  crates/mosaic-wasm/test/pipeline.test.ts
```

## Security

Mosaic runs untrusted, community-authored code by design. The trust boundaries and
guarantees, and how to report a vulnerability, are documented in
[`SECURITY.md`](./SECURITY.md).

## Contributing

See [`CONTRIBUTING.md`](./CONTRIBUTING.md). The bar is non-negotiable: every change
is tested and verified, `clippy` stays clean, and any change to the engine or a
Facet must preserve the native≡wasm determinism guarantee.

## License

Licensed under either of

- Apache License, Version 2.0 ([`LICENSE-APACHE`](./LICENSE-APACHE))
- MIT license ([`LICENSE-MIT`](./LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this work, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
