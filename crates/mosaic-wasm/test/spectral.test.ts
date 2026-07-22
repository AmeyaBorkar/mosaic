// The spectral (audio) browser pipeline, proven end-to-end against the native engine —
// the second engine reaching the same preview == render bar as the first. Two properties:
//   1. extract-in-wasm reproduces tessera-spectral's `feature::extract` bit-for-bit:
//      the Goertzel STFT is deterministic across the native and wasm builds (libm
//      sinf/cosf/powf agree), which is what makes a browser preview trustworthy.
//   2. the WHOLE pipeline (wasm spectral extract -> facet-ramp -> wasm compose) equals
//      the authoritative native `render_spectral` over every golden signal.
//
// The golden (native features + native render text) is emitted by
// `cargo run -p tessera-spectral --example emit_spectral_golden`. The Facet is the real
// `facet_ramp.wasm` — the SAME binary the image pipeline runs — through @mosaic/facet-abi.
// One Facet, two domains, on the browser path.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import { compileFacet, runFacetMap } from "../../../packages/facet-abi/src/index.ts";

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
  extract_spectral_features(
    samples: Float32Array,
    sampleRate: number,
    bands: number,
    win: number,
    hop: number,
    fmin: number,
    fmax: number,
  ): FeatureBuffer;
  compose(cols: number, rows: number, codepoints: Uint32Array): string;
}
const wasm = require("../pkg/mosaic_wasm.js") as MosaicWasm;

interface SpectralCase {
  name: string;
  samples: number[];
  gridCols: number;
  gridRows: number;
  stride: number;
  features: number[];
  text: string;
}
const golden = JSON.parse(readFileSync(here("./spectral_golden.json"), "utf8")) as {
  sampleRate: number;
  bands: number;
  win: number;
  hop: number;
  fmin: number;
  fmax: number;
  cases: SpectralCase[];
};
// The exact same Facet binary the image pipeline uses.
const rampWasm = readFileSync(
  here("../../../packages/facet-abi/test/fixtures/facet_ramp.wasm"),
);

function extract(c: SpectralCase): FeatureBuffer {
  return wasm.extract_spectral_features(
    Float32Array.from(c.samples),
    golden.sampleRate,
    golden.bands,
    golden.win,
    golden.hop,
    golden.fmin,
    golden.fmax,
  );
}

test("spectral extract-in-wasm reproduces native features bit-for-bit", () => {
  assert.ok(golden.cases.length >= 5, "expected the full spectral golden set");
  for (const c of golden.cases) {
    const fb = extract(c);
    try {
      assert.equal(fb.cols, c.gridCols, `${c.name}: cols`);
      assert.equal(fb.rows, c.gridRows, `${c.name}: rows`);
      assert.equal(fb.stride, c.stride, `${c.name}: stride`);
      // f32 round-trip: the golden decimals are the shortest that round-trip the native
      // f32; JSON widens to f64, so compare the exact-f32-as-f64 on both sides. A real
      // 1-ULP native-vs-wasm divergence in the Goertzel STFT still fails here.
      assert.deepEqual(
        Array.from(fb.data),
        Array.from(Float32Array.from(c.features)),
        `${c.name}: spectral features diverged from the native extract`,
      );
    } finally {
      fb.free();
    }
  }
});

test("full spectral browser pipeline (extract -> facet-ramp -> compose) matches native", async () => {
  const rampModule = await compileFacet(rampWasm);
  for (const c of golden.cases) {
    const fb = extract(c);
    try {
      const tokens = runFacetMap(rampModule, fb.data, fb.ncells, fb.stride);
      const text = wasm.compose(fb.cols, fb.rows, tokens);
      assert.equal(text, c.text, `${c.name}: spectral browser render != native render`);
    } finally {
      fb.free();
    }
  }
});
