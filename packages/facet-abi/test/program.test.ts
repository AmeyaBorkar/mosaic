// applyParams parity: patching a DSL program's params in the browser must (a) write the
// value at the same offset the native `mosaic_vm::patch_params` does, and (b) actually change
// the render when the real interpreter Facet runs the patched program — so a UI control can
// adjust a Facet live, without recompiling, and stay preview == render.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { applyParams, compileFacet, runFacetProgram, FacetAbiError } from "../src/index.ts";

const here = (p: string) => fileURLToPath(new URL(p, import.meta.url));

// The DSL golden's program is `grad_mag > threshold ? glyph(...) : ramp(...)` with one
// param (threshold, default 0.6).
const golden = JSON.parse(readFileSync(here("./dsl_golden.json"), "utf8")) as {
  program: number[];
  features: number[];
  ncells: number;
  stride: number;
};
const interp = new Uint8Array(readFileSync(here("./fixtures/facet_interp.wasm")));

test("applyParams writes the value at the params offset as little-endian f32", () => {
  const program = new Uint8Array(golden.program);
  const patched = applyParams(program, [0.42]);
  const value = new DataView(patched.buffer).getFloat32(10, true);
  assert.ok(Math.abs(value - 0.42) < 1e-6, "patched value round-trips");
  assert.equal(patched.length, program.length, "structure is unchanged");
});

test("a patched threshold changes the render through the real interpreter", async () => {
  const module = await compileFacet(interp);
  const program = new Uint8Array(golden.program);
  const features = Float32Array.from(golden.features);

  // threshold 0 -> grad_mag > 0 nearly always true (edge branch); 999 -> always false
  // (density branch). The two token streams must differ.
  const edges = runFacetProgram(module, applyParams(program, [0]), features, golden.ncells, golden.stride);
  const density = runFacetProgram(module, applyParams(program, [999]), features, golden.ncells, golden.stride);
  assert.notDeepEqual(
    Array.from(edges),
    Array.from(density),
    "threshold extremes must render differently",
  );
});

test("applyParams rejects the wrong value count", () => {
  const program = new Uint8Array(golden.program);
  assert.throws(() => applyParams(program, [1, 2, 3]), FacetAbiError);
});
