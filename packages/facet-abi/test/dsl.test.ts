// DSL browser parity (audit M4): the interpreter Facet, given a compiled bytecode
// program, must produce byte-identical tokens to the native `run_program` reference
// (dsl_golden.json), so "preview == render" extends to DSL-authored Facets — which
// previously had no browser execution path and always trapped.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { compileFacet, runFacetProgram } from "../src/host.ts";

const here = (p: string) => fileURLToPath(new URL(p, import.meta.url));
const interpWasm = readFileSync(here("./fixtures/facet_interp.wasm"));

interface DslGolden {
  program: number[];
  features: number[];
  ncells: number;
  stride: number;
  tokens: number[];
}
const golden = JSON.parse(
  readFileSync(here("./dsl_golden.json"), "utf8"),
) as DslGolden;

test("a DSL bytecode Facet renders in the browser matching native run_program", async () => {
  const module = await compileFacet(interpWasm);
  const tokens = runFacetProgram(
    module,
    Uint8Array.from(golden.program),
    Float32Array.from(golden.features),
    golden.ncells,
    golden.stride,
  );
  assert.deepEqual(Array.from(tokens), golden.tokens);
});

test("the interpreter rejects a malformed program (load_program returns nonzero)", async () => {
  const module = await compileFacet(interpWasm);
  const bad = Uint8Array.from([0, 1, 2, 3, 4, 5, 6, 7]); // wrong magic
  assert.throws(
    () => runFacetProgram(module, bad, Float32Array.from([0, 0, 0]), 1, 3),
    /rejected the bytecode program/,
  );
});
