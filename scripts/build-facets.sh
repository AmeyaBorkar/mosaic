#!/usr/bin/env bash
#
# Build every guest Facet from source to wasm32 and copy the output into each
# committed fixture location.
#
# The Facets are `exclude`d from the cargo workspace (they are no_std wasm guests), so
# `cargo build --workspace` never touches them and the committed `.wasm` fixtures — which
# every sandbox / conformance / golden test runs against — could otherwise drift from
# `facets/*/src`. CI runs this script and then `git diff --exit-code`s the fixtures, so a
# committed binary can never diverge from the source the "preview == render" proofs are
# measured against (audit H1). Run it yourself whenever a Facet's source, or a shared
# crate it links (glyph-atlas, dither, mosaic-vm), changes.
#
# RUSTFLAGS caps each Facet's linear memory at 16 MiB so the browser host enforces the
# same ceiling the native sandbox does; --locked builds against the committed Cargo.lock
# so the bytes are reproducible.
set -euo pipefail

cd "$(dirname "$0")/.."

export RUSTFLAGS="-C link-arg=--max-memory=16777216"

# Build one Facet and copy its single wasm output into every committed location.
build_and_copy() {
  local facet="$1"
  shift
  cargo build --locked --manifest-path "facets/$facet/Cargo.toml" \
    --target wasm32-unknown-unknown --release
  local src
  src="$(ls "facets/$facet/target/wasm32-unknown-unknown/release/"*.wasm | head -1)"
  local dest
  for dest in "$@"; do
    cp "$src" "$dest"
  done
}

build_and_copy ramp \
  crates/tessera-ascii/tests/facet_ramp.wasm \
  crates/tessera-spectral/tests/facet_ramp.wasm \
  crates/mosaic-wasm/tests/facet_ramp.wasm \
  crates/mosaic-compose/tests/facet_ramp.wasm \
  packages/facet-abi/test/fixtures/facet_ramp.wasm

build_and_copy dither \
  crates/tessera-ascii/tests/facet_dither.wasm \
  crates/tessera-spectral/tests/facet_dither.wasm \
  packages/facet-abi/test/fixtures/facet_dither.wasm

build_and_copy structural \
  crates/tessera-ascii/tests/facet_structural.wasm \
  packages/facet-abi/test/fixtures/facet_structural.wasm

build_and_copy interp \
  crates/mosaic-runtime/tests/facet_interp.wasm \
  crates/mosaic-certify/assets/facet_interp.wasm \
  packages/facet-abi/test/fixtures/facet_interp.wasm

build_and_copy liar \
  packages/facet-abi/test/fixtures/facet_liar.wasm

build_and_copy spin \
  packages/facet-abi/test/fixtures/facet_spin.wasm

echo "Facet wasm fixtures rebuilt from source."
