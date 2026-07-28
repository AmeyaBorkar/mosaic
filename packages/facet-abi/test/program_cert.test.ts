// DSL-program certificate parity: the browser verifier must reproduce, for a *certified*
// authored program, every probe outcome the authoritative host recorded in
// `program_cert_golden.json` (emitted by
// `cargo run -p mosaic-certify --example emit_program_cert_golden`). This extends
// "preview == render" (docs/architecture.md D9) from wasm Facets to DSL-authored Facets, and
// proves the program-hash binding and mismatch detection. The programs run on the *same*
// interpreter fixture the golden was produced against.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  compileFacet,
  verifyProgramCertificate,
  FacetAbiError,
  type ProgramCertificate,
} from "../src/index.ts";

const here = (p: string) => fileURLToPath(new URL(p, import.meta.url));

interface GoldenCase {
  source: string;
  program: number[];
  certificate: ProgramCertificate;
}
const golden = JSON.parse(
  readFileSync(here("./program_cert_golden.json"), "utf8"),
) as { facetWasm: string; cases: GoldenCase[] };

// The shared interpreter, compiled once (as a UI would) and reused across verifications.
const interp = await compileFacet(new Uint8Array(readFileSync(here(`./${golden.facetWasm}`))));

for (const c of golden.cases) {
  test(`verifyProgramCertificate reproduces the certificate for: ${c.source}`, async () => {
    const program = Uint8Array.from(c.program);
    await verifyProgramCertificate(interp, program, c.certificate); // resolves iff every probe matches
  });
}

test("verifyProgramCertificate rejects a tampered token", async () => {
  const c = structuredClone(golden.cases[0]!);
  const program = Uint8Array.from(c.program);
  const probe = c.certificate.probes.find((p) => p.outcome.result === "tokens");
  if (!probe || probe.outcome.result !== "tokens") {
    throw new Error("expected a tokens probe in the golden");
  }
  probe.outcome.tokens[0] = (probe.outcome.tokens[0]! ^ 1) >>> 0;
  await assert.rejects(
    () => verifyProgramCertificate(interp, program, c.certificate),
    FacetAbiError,
  );
});

test("verifyProgramCertificate rejects a program-hash mismatch", async () => {
  const c = structuredClone(golden.cases[0]!);
  const program = Uint8Array.from(c.program);
  c.certificate.programSha256 = "0".repeat(64);
  await assert.rejects(
    () => verifyProgramCertificate(interp, program, c.certificate),
    FacetAbiError,
  );
});

test("verifyProgramCertificate detects a tampered program (patched bytecode)", async () => {
  // Flip a param byte in the program: the hash no longer matches its certificate.
  const c = structuredClone(golden.cases[0]!);
  const program = Uint8Array.from(c.program);
  program[10] = program[10]! ^ 0xff; // first param byte (offset 10)
  await assert.rejects(
    () => verifyProgramCertificate(interp, program, c.certificate),
    FacetAbiError,
  );
});
