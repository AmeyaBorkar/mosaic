# @mosaic/facet-abi

The browser-side host for Mosaic **Facets** — untrusted, community-authored WASM
that turns a per-cell feature buffer into per-cell output tokens.

On the server, Facets run on wasmtime with fuel metering and `StoreLimits`
(`mosaic-runtime`). The browser has no wasmtime and core WebAssembly has **no
fuel**, so this package provides the browser half of decision **D9**
(`docs/architecture.md`):

- **Same ABI, verified.** `runFacetMap` mirrors `mosaic-runtime::Sandbox::run_map`
  byte-for-byte: identical stride/length/overflow checks, allocate-through-the-
  guest marshalling, little-endian `f32` in / `u32` tokens out. A golden vector
  emitted by the proven native host pins the two implementations together, so
  **preview == render** is a *tested* property, not an assumption.
- **Purity is structural.** Modules instantiate with zero imports and any module
  that *declares* an import is rejected up front (`validateFacetModule`).
- **Metering without fuel.** `runFacetSandboxed` runs the Facet in a Web Worker
  under a wall-clock timeout; an overrunning or memory-bombing Facet is
  `terminate()`d and surfaces a clean error instead of freezing the page.

## API

```ts
import { compileFacet, runFacetMap, runFacetSandboxed } from "@mosaic/facet-abi";

// Fast path (trusted context / already-vetted Facet): synchronous, in-thread.
const module = await compileFacet(facetBytes);       // compiles + validates
const tokens = runFacetMap(module, features, ncells, stride); // Uint32Array

// Untrusted path (live preview of author code): isolated + timeout-metered.
const tokens = await runFacetSandboxed(facetBytes, features, ncells, stride, {
  timeoutMs: 250,
});
```

Token → text composition is intentionally **not** here: it belongs to the engine
(`mosaic-wasm`, the same Rust `compose` used server-side) so a malicious token is
validated by one implementation, not two.

## Tests

```
node --test test/conformance.test.ts test/adversarial.test.ts test/timeout.test.ts
```

Fixtures and the golden vector are regenerated from Rust with:

```
cargo run -p mosaic-runtime --example emit_golden       # golden.json + facet_ramp.wasm
cargo build --manifest-path facets/spin/Cargo.toml --target wasm32-unknown-unknown --release
cargo build --manifest-path facets/liar/Cargo.toml --target wasm32-unknown-unknown --release
```
