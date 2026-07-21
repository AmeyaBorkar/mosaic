// Conformance: the browser-side host must reproduce the native `run_map` tokens
// exactly. The golden vector in `golden.json` was emitted by the proven native
// host (`cargo run -p mosaic-runtime --example emit_golden`) running the *same*
// `facet_ramp.wasm` this test loads. Matching it byte-for-byte is what makes
// "preview == render" (docs/architecture.md D9) a checked property.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { compileFacet, runFacetMap } from "../src/index.ts";

const here = (p) => fileURLToPath(new URL(p, import.meta.url));

interface GoldenCase {
  name: string;
  stride: number;
  ncells: number;
  features: number[];
  tokens: number[];
}
const golden = JSON.parse(readFileSync(here("./golden.json"), "utf8")) as {
  cases: GoldenCase[];
};
const rampWasm = readFileSync(here("./fixtures/facet_ramp.wasm"));

test("browser host reproduces native run_map tokens for every golden case", async () => {
  const module = await compileFacet(rampWasm);
  assert.ok(golden.cases.length >= 5, "expected the full golden set");
  for (const c of golden.cases) {
    const features = Float32Array.from(c.features);
    const tokens = runFacetMap(module, features, c.ncells, c.stride);
    assert.deepEqual(
      Array.from(tokens),
      c.tokens,
      `case '${c.name}' diverged from the native oracle`,
    );
  }
});

test("re-running a Facet is deterministic (same tokens every time)", async () => {
  const module = await compileFacet(rampWasm);
  const c = golden.cases[0]!;
  const features = Float32Array.from(c.features);
  const first = runFacetMap(module, features, c.ncells, c.stride);
  for (let i = 0; i < 5; i++) {
    const again = runFacetMap(module, features, c.ncells, c.stride);
    assert.deepEqual(Array.from(again), Array.from(first));
  }
});
