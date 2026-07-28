// The full in-browser authoring loop, proven end to end — the reference a Facet editor UI
// wires:
//   compile DSL (wasm.compileDsl) -> extract features (wasm.extract_features)
//   -> run the bytecode in the sandbox (facet-abi runFacetProgram) -> compose text
//   (wasm.compose), plus LIVE CONTROLS via applyParams (patch a param, re-run, no recompile).
// Everything is browser-local, and the DSL compiler here is the SAME mosaic-dsl the server
// runs, so the whole loop stays preview == render.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import {
  compileFacet,
  runFacetProgram,
  applyParams,
} from "../../../packages/facet-abi/src/index.ts";

const require = createRequire(import.meta.url);
const here = (p) => fileURLToPath(new URL(p, import.meta.url));
const wasm = require("../pkg/mosaic_wasm.js");

const interpBytes = new Uint8Array(
  readFileSync(here("../../../packages/facet-abi/test/fixtures/facet_interp.wasm")),
);

// A horizontal luminance ramp, so gradients — and thus the threshold — actually vary.
function rampImage(w, h) {
  const rgba = new Uint8Array(w * h * 4);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const v = Math.round((x / (w - 1)) * 255);
      const i = (y * w + x) * 4;
      rgba[i] = v;
      rgba[i + 1] = v;
      rgba[i + 2] = v;
      rgba[i + 3] = 255;
    }
  }
  return rgba;
}

// The same branchy Facet the DSL golden uses.
const SRC =
  'grad_mag > threshold ? glyph(clamp(grad_dir * 1.27 + 2.0, 0, 3), "-/|\\\\") : ramp(luma, " .:-=+*#%@")';
const PARAMS = JSON.stringify([{ name: "threshold", value: 0.6 }]);

test("compile DSL -> extract -> run -> compose yields a text grid + a control manifest", async () => {
  // 1. Author text compiles to bytecode in the browser (same crate as the server).
  const compiled = wasm.compileDsl("ascii", SRC, PARAMS);
  const manifest = JSON.parse(compiled.manifestJson);
  assert.equal(manifest.engine, "ascii");
  assert.equal(manifest.params.length, 1);
  assert.equal(manifest.params[0].name, "threshold");
  assert.equal(manifest.params[0].index, 0);

  // 2. Extract features from the image (wasm), 3. run the program in the sandbox (facet-abi).
  const module = await compileFacet(interpBytes);
  const [w, h] = [16, 16];
  const fb = wasm.extract_features(rampImage(w, h), w, h, 8, 2.0);
  const features = Float32Array.from(fb.data);
  const { cols, rows, ncells, stride } = fb;
  fb.free();

  const tokens = runFacetProgram(module, compiled.program, features, ncells, stride);

  // 4. Compose the tokens to text (wasm) — the shared, untrusted-glyph-safe composer.
  const textOut = wasm.compose(cols, rows, tokens);
  assert.equal(textOut.split("\n").length, rows);
  assert.ok(textOut.length > 0);
});

test("a noise Facet compiles and runs in the browser, scattering glyphs across the grid", async () => {
  // `noise(u, v)` — the HASH opcode — authored, compiled by the browser's own mosaic-dsl
  // (compileDsl), and run in the sandbox, exactly as a UI would. Proves the browser compiler
  // accepts the builtin and the interp executes the opcode.
  const compiled = wasm.compileDsl("ascii", 'ramp(noise(u, v), " .:-=+*#%@")', "[]");
  const module = await compileFacet(interpBytes);
  const [w, h] = [16, 16];
  const fb = wasm.extract_features(rampImage(w, h), w, h, 8, 2.0);
  const features = Float32Array.from(fb.data);
  const { ncells, stride } = fb;
  fb.free();

  const tokens = runFacetProgram(module, compiled.program, features, ncells, stride);
  // Noise scatters glyphs across the ramp, so several distinct glyphs appear even on this
  // smooth image — breaking up banding is the whole point.
  assert.ok(new Set(tokens).size > 3, "expected varied noise glyphs");
  // Deterministic: identical inputs reproduce identical tokens.
  const again = runFacetProgram(module, compiled.program, features, ncells, stride);
  assert.deepEqual(Array.from(again), Array.from(tokens));
});

test("a live control (applyParams) changes the render without recompiling", async () => {
  const compiled = wasm.compileDsl("ascii", SRC, PARAMS);
  const module = await compileFacet(interpBytes);
  const [w, h] = [16, 16];
  const fb = wasm.extract_features(rampImage(w, h), w, h, 8, 2.0);
  const features = Float32Array.from(fb.data);
  const { ncells, stride } = fb;
  fb.free();

  // threshold 0 -> edge branch nearly everywhere; 999 -> density branch everywhere.
  const edges = runFacetProgram(module, applyParams(compiled.program, [0]), features, ncells, stride);
  const density = runFacetProgram(module, applyParams(compiled.program, [999]), features, ncells, stride);
  assert.notDeepEqual(
    Array.from(edges),
    Array.from(density),
    "adjusting the threshold control must change the render",
  );
});
