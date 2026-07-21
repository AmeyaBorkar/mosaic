# Security Policy

Mosaic executes **untrusted, community-authored code** (Facets) by design, both on
the server and in users' browsers. Security is therefore a first-class,
load-bearing property of the architecture — not an afterthought.

## Threat model

A Facet is arbitrary WebAssembly supplied by an untrusted author. The platform
must contain it. The guarantees:

- **Purity is structural.** A Facet is instantiated with **zero imports**; a module
  that so much as *declares* an import is rejected before it can run. It has no
  ambient authority — no network, disk, clock, or randomness.
- **Metering.** Server-side, execution is bounded by wasmtime **fuel** and by
  `StoreLimits` caps on linear memory, tables, table elements, memories, and
  instances; module size is bounded before compilation. In the browser (which has
  no fuel), a Facet runs inside a **Web Worker** under a wall-clock **timeout** and
  is forcibly terminated if it overruns; a memory bomb is contained to the worker.
- **Determinism.** NaN payloads are canonicalized and relaxed-SIMD / threads /
  multi-memory are disabled, so execution is bit-identical across machines. Engine
  transcendentals use `libm` for cross-platform reproducibility.
- **Bounds-checked marshalling.** Every host↔guest crossing is bounds-checked and
  every size derived from untrusted input is overflow-checked before any
  allocation, on both the native (`mosaic-runtime`) and browser (`@mosaic/facet-abi`)
  hosts. Untrusted output codepoints are validated before composition.

These properties are covered by an adversarial test suite (memory/table bombs,
import smuggling, wild pointers, infinite loops, malformed exports) and by
native≡wasm conformance sweeps.

## Reporting a vulnerability

**Please do not open a public issue for security vulnerabilities.**

Report privately via GitHub's **[Security Advisories](https://github.com/AmeyaBorkar/mosaic/security/advisories/new)**
("Report a vulnerability"). Include a description, affected component, and a
reproduction if possible. We aim to acknowledge within a few days and will
coordinate a fix and disclosure.

If a sandbox-escape or a preview≠render (determinism) divergence is found, treat it
as high severity — both are core invariants of the platform.

## Scope

In scope: sandbox escape, resource-exhaustion that escapes the configured caps,
host memory corruption via the Facet ABI, and determinism breaks (preview ≠
render). Out of scope (for now): the not-yet-built registry, DSL, and web UI.
