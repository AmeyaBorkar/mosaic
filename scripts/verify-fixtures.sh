#!/usr/bin/env bash
#
# Regenerate the golden conformance vectors from source and fail if the committed
# copies are stale.
#
# The goldens are computed from deterministic f32 arithmetic (libm + IEEE-754) and
# the committed Facet wasm, so they reproduce byte-for-byte across machines. A diff
# here means the engine or a Facet changed without the goldens being refreshed —
# which would silently weaken the "preview == render" guarantee.
set -euo pipefail

cargo run --quiet -p mosaic-runtime   --example emit_golden
cargo run --quiet -p tessera-ascii    --example emit_render_golden
cargo run --quiet -p tessera-spectral --example emit_spectral_golden

if ! git diff --exit-code -- \
    packages/facet-abi/test/golden.json \
    crates/mosaic-wasm/test/render_golden.json \
    crates/mosaic-wasm/test/spectral_golden.json; then
  echo "::error::Golden vectors are stale. Re-run the emit_* examples and commit the result." >&2
  exit 1
fi

echo "Golden vectors are fresh."
