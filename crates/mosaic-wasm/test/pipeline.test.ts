// The concrete browser render pipeline, proven end-to-end against the native
// engine. Three properties:
//   1. extract-in-wasm reproduces `feature::extract` bit-for-bit (cross-compile
//      determinism, incl. libm::atan2f native vs wasm) — the basis of preview==render.
//   2. compose-in-wasm validates untrusted codepoints exactly like the native engine.
//   3. the WHOLE pipeline (wasm extract -> facet-abi Facet -> wasm compose) equals
//      the authoritative native `render_ascii` over every golden image.
//
// The golden (native features + native render text) is emitted by
// `cargo run -p tessera-ascii --example emit_render_golden`. The Facet is the real
// `facet_ramp.wasm`, run through @mosaic/facet-abi (the browser Facet host).

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import {
  compileFacet,
  runFacetMap,
  runFacetMap2d,
} from "../../../packages/facet-abi/src/index.ts";

const require = createRequire(import.meta.url);
const here = (p) => fileURLToPath(new URL(p, import.meta.url));

interface FeatureBuffer {
  readonly cols: number;
  readonly rows: number;
  readonly stride: number;
  readonly ncells: number;
  readonly data: Float32Array;
  free(): void;
}
interface MosaicWasm {
  extract_features(
    rgba: Uint8Array,
    width: number,
    height: number,
    cols: number,
    cellAspect: number,
  ): FeatureBuffer;
  extract_structural_features(
    rgba: Uint8Array,
    width: number,
    height: number,
    cols: number,
    cellAspect: number,
  ): FeatureBuffer;
  compose(cols: number, rows: number, codepoints: Uint32Array): string;
}
const wasm = require("../pkg/mosaic_wasm.js") as MosaicWasm;

interface RenderCase {
  name: string;
  width: number;
  height: number;
  cols: number;
  gridCols: number;
  gridRows: number;
  stride: number;
  rgba: number[];
  features: number[];
  text: string;
  structuralText: string;
  ditherText: string;
}
const golden = JSON.parse(readFileSync(here("./render_golden.json"), "utf8")) as {
  cellAspect: number;
  cases: RenderCase[];
};
const rampWasm = readFileSync(
  here("../../../packages/facet-abi/test/fixtures/facet_ramp.wasm"),
);
const structuralWasm = readFileSync(
  here("../../../packages/facet-abi/test/fixtures/facet_structural.wasm"),
);
const ditherWasm = readFileSync(
  here("../../../packages/facet-abi/test/fixtures/facet_dither.wasm"),
);

test("extract-in-wasm reproduces native features bit-for-bit", () => {
  assert.ok(golden.cases.length >= 8, "expected the full golden set");
  for (const c of golden.cases) {
    const fb = wasm.extract_features(
      Uint8Array.from(c.rgba),
      c.width,
      c.height,
      c.cols,
      golden.cellAspect,
    );
    try {
      assert.equal(fb.cols, c.gridCols, `${c.name}: cols`);
      assert.equal(fb.rows, c.gridRows, `${c.name}: rows`);
      assert.equal(fb.stride, c.stride, `${c.name}: stride`);
      // Round the golden decimals back through f32 (they are the shortest decimal
      // that round-trips the native f32; JSON.parse widens them to f64). Both sides
      // are then the exact f64-of-the-f32, so a real 1-ULP divergence still fails.
      assert.deepEqual(
        Array.from(fb.data),
        Array.from(Float32Array.from(c.features)),
        `${c.name}: features diverged from the native extract`,
      );
    } finally {
      fb.free();
    }
  }
});

test("compose validates untrusted codepoints exactly like the native engine", () => {
  // 'A', a surrogate (invalid), out-of-range (invalid), 'B' -> replacements.
  assert.equal(
    wasm.compose(2, 2, Uint32Array.from([0x41, 0xd800, 0x11_0000, 0x42])),
    "A�\n�B",
  );
  // Too few codepoints: missing cells become the replacement char, no panic.
  assert.equal(wasm.compose(2, 1, Uint32Array.from([0x41])), "A�");
});

test("full browser pipeline (extract -> Facet -> compose) matches the native render", async () => {
  const rampModule = await compileFacet(rampWasm);
  for (const c of golden.cases) {
    const fb = wasm.extract_features(
      Uint8Array.from(c.rgba),
      c.width,
      c.height,
      c.cols,
      golden.cellAspect,
    );
    try {
      const tokens = runFacetMap(rampModule, fb.data, fb.ncells, fb.stride);
      const text = wasm.compose(fb.cols, fb.rows, tokens);
      assert.equal(text, c.text, `${c.name}: browser render != native render`);
    } finally {
      fb.free();
    }
  }
});

test("full structural (L2) browser pipeline matches the native render", async () => {
  const structuralModule = await compileFacet(structuralWasm);
  for (const c of golden.cases) {
    const fb = wasm.extract_structural_features(
      Uint8Array.from(c.rgba),
      c.width,
      c.height,
      c.cols,
      golden.cellAspect,
    );
    try {
      assert.equal(fb.stride, 64, `${c.name}: structural stride`);
      const tokens = runFacetMap(structuralModule, fb.data, fb.ncells, fb.stride);
      const text = wasm.compose(fb.cols, fb.rows, tokens);
      assert.equal(text, c.structuralText, `${c.name}: structural browser render != native`);
    } finally {
      fb.free();
    }
  }
});

test("full dither (propagation) browser pipeline matches the native render", async () => {
  const ditherModule = await compileFacet(ditherWasm);
  for (const c of golden.cases) {
    const fb = wasm.extract_features(
      Uint8Array.from(c.rgba),
      c.width,
      c.height,
      c.cols,
      golden.cellAspect,
    );
    try {
      // The 2-D ABI: hand the Facet the grid shape so its feedback loop can address
      // neighbours. L0 luminance is slot 0 of the stride-3 buffer.
      const tokens = runFacetMap2d(ditherModule, fb.data, fb.cols, fb.rows, fb.stride);
      const text = wasm.compose(fb.cols, fb.rows, tokens);
      assert.equal(text, c.ditherText, `${c.name}: dither browser render != native`);
    } finally {
      fb.free();
    }
  }
});
