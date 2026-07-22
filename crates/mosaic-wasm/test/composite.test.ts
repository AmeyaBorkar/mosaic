// Browser composition (O4) conformance: the WasmCanvas binding reproduces native
// mosaic-core::composite byte-for-byte. For each golden case — real cross-engine layer
// data (image ASCII + audio spectrogram, produced natively via the sandboxed facet-ramp)
// — rebuild the composition from the stored layers through Canvas.place and assert
// into_text equals the native composed text.
//
// Combined with the pipeline/spectral goldens (browser extract + Facet == native tokens),
// this closes the chain: the browser produces a cross-engine artifact identical to the
// authoritative server render. Composition is pure integer/Bayer grid logic, so any
// divergence would be a real bug, not float drift.
//
// The golden is emitted by `cargo run -p mosaic-wasm --example emit_composite_golden`.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const here = (p) => fileURLToPath(new URL(p, import.meta.url));

interface WasmCanvas {
  place(
    tokens: Uint32Array,
    coverage: Float32Array,
    layerCols: number,
    layerRows: number,
    rowOff: number,
    colOff: number,
    blend: string,
  ): void;
  into_text(background: number): string;
}
interface MosaicWasm {
  Canvas: new (cols: number, rows: number) => WasmCanvas;
}
const wasm = require("../pkg/mosaic_wasm.js") as MosaicWasm;

interface LayerSpec {
  cols: number;
  rows: number;
  rowOff: number;
  colOff: number;
  blend: string;
  tokens: number[];
  coverage: number[];
}
interface CompositeCase {
  name: string;
  canvas: { cols: number; rows: number };
  background: number;
  layers: LayerSpec[];
  text: string;
}
const golden = JSON.parse(readFileSync(here("./composite_golden.json"), "utf8")) as {
  cases: CompositeCase[];
};

test("browser Canvas reproduces native composition byte-for-byte", () => {
  assert.ok(golden.cases.length >= 2, "expected the full composite golden set");
  for (const c of golden.cases) {
    const canvas = new wasm.Canvas(c.canvas.cols, c.canvas.rows);
    for (const l of c.layers) {
      canvas.place(
        Uint32Array.from(l.tokens),
        Float32Array.from(l.coverage),
        l.cols,
        l.rows,
        l.rowOff,
        l.colOff,
        l.blend,
      );
    }
    const text = canvas.into_text(c.background);
    assert.equal(text, c.text, `${c.name}: browser composition != native`);
  }
});

test("an unknown blend mode is rejected, not silently ignored", () => {
  const canvas = new wasm.Canvas(2, 2);
  assert.throws(
    () =>
      canvas.place(
        Uint32Array.from([65, 65, 65, 65]),
        Float32Array.from([1, 1, 1, 1]),
        2,
        2,
        0,
        0,
        "bogus",
      ),
    /unknown blend/,
  );
});
