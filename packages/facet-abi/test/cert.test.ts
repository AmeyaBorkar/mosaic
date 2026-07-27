// Certificate parity: the browser verifier must reproduce, for a *certified* Facet, every
// probe outcome the authoritative host recorded in `cert_golden.json` (emitted by
// `cargo run -p mosaic-certify --example emit_cert_golden`). This extends "preview ==
// render" (docs/architecture.md D9) from the hand-authored goldens to an arbitrary
// certified Facet, and proves the hash binding and mismatch detection.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { verifyCertificate, FacetAbiError, type Certificate } from "../src/index.ts";

const here = (p: string) => fileURLToPath(new URL(p, import.meta.url));

interface GoldenCase {
  facetWasm: string;
  certificate: Certificate;
}
const golden = JSON.parse(readFileSync(here("./cert_golden.json"), "utf8")) as {
  cases: GoldenCase[];
};

for (const c of golden.cases) {
  test(`verifyCertificate reproduces the certificate for ${c.facetWasm}`, async () => {
    const bytes = new Uint8Array(readFileSync(here(`./${c.facetWasm}`)));
    await verifyCertificate(bytes, c.certificate); // resolves iff every probe matches
  });
}

test("verifyCertificate rejects a tampered token", async () => {
  const c = structuredClone(golden.cases[0]!);
  const bytes = new Uint8Array(readFileSync(here(`./${c.facetWasm}`)));
  const probe = c.certificate.probes.find((p) => p.outcome.result === "tokens");
  if (!probe || probe.outcome.result !== "tokens") {
    throw new Error("expected a tokens probe in the golden");
  }
  probe.outcome.tokens[0] = (probe.outcome.tokens[0]! ^ 1) >>> 0;
  await assert.rejects(() => verifyCertificate(bytes, c.certificate), FacetAbiError);
});

test("verifyCertificate rejects a hash mismatch", async () => {
  const c = structuredClone(golden.cases[0]!);
  const bytes = new Uint8Array(readFileSync(here(`./${c.facetWasm}`)));
  c.certificate.wasmSha256 = "0".repeat(64);
  await assert.rejects(() => verifyCertificate(bytes, c.certificate), FacetAbiError);
});

test("the certificate golden exercises both ABI kinds", () => {
  const kinds = new Set(golden.cases.map((c) => c.certificate.abiKind));
  assert.ok(
    kinds.has("gather") && kinds.has("propagation"),
    "golden should certify a gather and a propagation Facet",
  );
});
