// Adversarial: the browser host must reject malformed or hostile Facets with a
// clean error, never a host crash or an out-of-bounds access — the browser-side
// mirror of mosaic-runtime's adversarial suite. Structural checks use tiny
// hand-encoded modules (asserted to be valid wasm first, so a bad byte fails
// loudly rather than passing vacuously); the wild-pointer check uses a real
// compiled Facet fixture.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  compileFacet,
  runFacetMap,
  validateFacetModule,
  checkedWasmByteLen,
  FacetAbiError,
  MAX_WASM_LEN,
} from "../src/index.ts";

const here = (p) => fileURLToPath(new URL(p, import.meta.url));
const rampWasm = readFileSync(here("./fixtures/facet_ramp.wasm"));
const liarWasm = readFileSync(here("./fixtures/facet_liar.wasm"));

const HEADER = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

// A module that declares an import `e.f` (func) and nothing else.
const IMPORTS_MODULE = new Uint8Array([
  ...HEADER,
  0x01, 0x04, 0x01, 0x60, 0x00, 0x00, // type section: one () -> ()
  0x02, 0x07, 0x01, 0x01, 0x65, 0x01, 0x66, 0x00, 0x00, // import "e"."f" func 0
]);
// Header only: valid wasm, exports nothing.
const EMPTY_MODULE = new Uint8Array([...HEADER]);
// Exports `memory` but neither `alloc` nor `run`.
const MEMORY_ONLY_MODULE = new Uint8Array([
  ...HEADER,
  0x05, 0x03, 0x01, 0x00, 0x01, // memory section: one memory {min 1}
  0x07, 0x0a, 0x01, 0x06, 0x6d, 0x65, 0x6d, 0x6f, 0x72, 0x79, 0x02, 0x00, // export "memory"
]);

test("hand-encoded adversarial modules are valid wasm (no vacuous passes)", async () => {
  await assert.doesNotReject(WebAssembly.compile(IMPORTS_MODULE));
  await assert.doesNotReject(WebAssembly.compile(EMPTY_MODULE));
  await assert.doesNotReject(WebAssembly.compile(MEMORY_ONLY_MODULE));
});

test("rejects a Facet that declares any import (purity is structural)", async () => {
  const module = await WebAssembly.compile(IMPORTS_MODULE);
  assert.throws(() => validateFacetModule(module), /zero imports/);
});

test("rejects a Facet missing the 'memory' export", async () => {
  const module = await WebAssembly.compile(EMPTY_MODULE);
  assert.throws(() => validateFacetModule(module), /export 'memory'/);
});

test("rejects a Facet missing a required function export", async () => {
  const module = await WebAssembly.compile(MEMORY_ONLY_MODULE);
  assert.throws(() => validateFacetModule(module), /export 'alloc'/);
});

test("rejects zero stride before doing anything else", async () => {
  const module = await compileFacet(rampWasm);
  assert.throws(() => runFacetMap(module, new Float32Array(3), 3, 0), /stride/);
});

test("rejects a feature length that disagrees with ncells * stride", async () => {
  const module = await compileFacet(rampWasm);
  assert.throws(
    () => runFacetMap(module, new Float32Array(5), 2, 3),
    /features length/,
  );
});

test("bounds an untrusted size before allocating", () => {
  assert.equal(checkedWasmByteLen(1000, 4), 4000);
  assert.equal(MAX_WASM_LEN, 0x7fffffff);
  assert.throws(() => checkedWasmByteLen(0x20000000, 4), /exceeds 32-bit/);
  assert.throws(() => checkedWasmByteLen(-1, 4), /invalid element count/);
});

test("rejects a Facet whose alloc returns a wild pointer (bounds-checked)", async () => {
  const module = await compileFacet(liarWasm);
  assert.throws(
    () => runFacetMap(module, Float32Array.from([0.5]), 1, 1),
    (e) =>
      e instanceof FacetAbiError &&
      /invalid pointer|out of bounds/.test(e.message),
  );
});
