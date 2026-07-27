// Coloured output in the browser: renderHalfblock and extractColors expose
// tessera_ascii::color, so a UI can paint coloured pixel art (▀ with fg over bg) and tinted
// glyphs. Colour is a deterministic integer mean from the source image, so it matches the
// native engine (these expectations mirror the native `tessera_ascii::color` tests).

import test from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const wasm = require("../pkg/mosaic_wasm.js");

// 2x2: top row red, bottom row blue.
function image() {
  return new Uint8Array([
    255, 0, 0, 255, 255, 0, 0, 255, // y=0 red
    0, 0, 255, 255, 0, 0, 255, 255, // y=1 blue
  ]);
}
const packRgba = (r, g, b, a) => (r | (g << 8) | (b << 16) | (a << 24)) >>> 0;

test("renderHalfblock splits the cell into top (fg) and bottom (bg) colours", () => {
  const hb = wasm.renderHalfblock(image(), 2, 2, 1, 1.0);
  assert.equal(hb.cols, 1);
  assert.equal(hb.rows, 1);
  assert.equal(hb.glyph, 0x2580); // ▀
  assert.deepEqual(Array.from(hb.fg), [packRgba(255, 0, 0, 255)]); // top: red
  assert.deepEqual(Array.from(hb.bg), [packRgba(0, 0, 255, 255)]); // bottom: blue
});

test("extractColors returns the per-cell integer mean colour", () => {
  const colors = wasm.extractColors(image(), 2, 2, 1, 1.0);
  assert.deepEqual(Array.from(colors), [packRgba(127, 0, 127, 255)]);
});
