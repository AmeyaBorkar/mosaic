# Contributing to Mosaic

Mosaic is treated as production. Contributions are welcome, held to a firm bar.

## The bar

Every change must:

1. **Build and pass** — `cargo test --workspace` and the JS suite are green.
2. **Be `clippy`-clean** — `cargo clippy --all-targets --all-features -- -D warnings`
   with no warnings, and `cargo fmt --all` applied.
3. **Preserve determinism** — any change to an engine (`tessera-ascii`,
   `glyph-atlas`) or a Facet must keep the native render bit-identical to the wasm
   preview. New float math must be deterministic across the native and wasm targets
   (route transcendentals through `libm`; no `fma` contraction or NaN-dependent
   branches). The conformance tests and golden vectors enforce this.
4. **Be covered** — new behavior comes with tests; new untrusted-input paths come
   with an adversarial test.

## Development setup

See the "Getting started" section of the [README](./README.md) for prerequisites
and commands. In short:

```sh
cargo test --workspace
cargo clippy --all-targets --all-features -- -D warnings
# browser bridge + JS tests:
wasm-pack build crates/mosaic-wasm --target nodejs --dev --out-dir pkg
node --test packages/facet-abi/test/*.test.ts crates/mosaic-wasm/test/pipeline.test.ts
```

## Regenerating fixtures and goldens

The guest Facet wasm files under `crates/tessera-ascii/tests/` and
`packages/facet-abi/test/fixtures/` are committed test inputs. If you change a
Facet's source, rebuild it and refresh the golden vectors, then commit the result:

```sh
# The RUSTFLAGS caps the Facet's linear memory at 16 MiB (browser parity with the
# native sandbox cap); required for @mosaic/facet-abi to accept the module.
RUSTFLAGS="-C link-arg=--max-memory=16777216" \
  cargo build --manifest-path facets/<name>/Cargo.toml --target wasm32-unknown-unknown --release
# copy the wasm into the fixture locations, then:
cargo run -p mosaic-runtime --example emit_golden
cargo run -p tessera-ascii  --example emit_render_golden
```

CI regenerates the goldens from source and fails if the committed copies are stale
(`scripts/verify-fixtures.sh`).

## Commits and history

- Keep commits **atomic and bisectable** — each commit builds and passes on its
  own. Prefer a series of focused commits over one large one.
- Use clear, conventional-style messages (`feat(scope): …`, `fix(scope): …`,
  `chore: …`, `ci: …`). Explain *why* in the body when it isn't obvious.
- Rebase to keep history linear; no merge commits on `main`.

## Architecture decisions

Significant design choices are recorded as numbered decisions (D1–D9) in
[`docs/architecture.md`](./docs/architecture.md). If you change one or add a new
one, update that document in the same change.
