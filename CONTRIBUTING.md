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
# browser bridge + JS tests (+ strict type-check):
wasm-pack build crates/mosaic-wasm --target nodejs --dev --out-dir pkg
pnpm install && pnpm run typecheck && pnpm test
```

## Regenerating fixtures and goldens

The guest Facet wasm files are committed test inputs, copied into several fixture
directories under `crates/*/tests/` and `packages/facet-abi/test/fixtures/`. Both the
placement and the goldens are script-driven, so do not hand-copy or hand-edit them. If you
change a Facet's source, or a shared crate it links (`glyph-atlas`, `dither`, `mosaic-vm`),
rebuild and refresh, then commit the result:

```sh
# Builds every Facet from source with the 16 MiB --max-memory cap and copies each into
# all of its committed fixture locations.
bash scripts/build-facets.sh
# Regenerates and verifies the five golden vectors.
bash scripts/verify-fixtures.sh
```

CI rebuilds every Facet from source and `git diff`s the committed `.wasm`, and regenerates
the goldens and fails if either is stale — so a committed binary can never drift from its
source.

## Commits and history

- Keep commits **atomic and bisectable** — each commit builds and passes on its
  own. Prefer a series of focused commits over one large one.
- Use clear, conventional-style messages (`feat(scope): …`, `fix(scope): …`,
  `chore: …`, `ci: …`). Explain *why* in the body when it isn't obvious.
- Rebase to keep history linear; no merge commits on `main`.

## Architecture decisions

Significant design choices are recorded as numbered decisions (D1–D14) in
[`docs/architecture.md`](./docs/architecture.md). If you change one or add a new
one, update that document in the same change.
