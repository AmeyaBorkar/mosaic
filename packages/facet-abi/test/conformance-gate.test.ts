// Conformance gate — browser structural pre-checks (defense in depth for the
// server-authoritative admission model). A Facet is only admitted if the native `wasmtime`
// accepts it (statically rejecting relaxed-SIMD, threads, and multi-memory) and its tokens
// match across hosts over a canonical battery. These checks let the browser reject the same
// classes early, so it never previews a Facet the server would refuse.
//
// checkMemoryLimits parses the raw module bytes (WebAssembly.Module reflection does not
// expose memory limits), so we exercise it directly with hand-crafted memory sections.

import test from "node:test";
import assert from "node:assert/strict";
import { checkMemoryLimits } from "../src/host.ts";

const MAGIC = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]; // "\0asm" + version 1

/** A minimal module: the 8-byte header followed by a raw section (id, size, body…). */
function mod(...section: number[]): Uint8Array {
  return Uint8Array.from([...MAGIC, ...section]);
}

// Memory section (id 5): [id, size, count, entries…]; entry = [flags, min, (max)?].
// flags: bit0 = has-max, bit1 = shared.

test("rejects more than one linear memory (native sandbox disables multi-memory)", () => {
  // count=2, two bounded [1,1] memories.
  const twoMems = mod(5, 7, 2, 0x01, 1, 1, 0x01, 1, 1);
  assert.throws(() => checkMemoryLimits(twoMems), /at most one linear memory/);
});

test("rejects shared memory (threads are disabled)", () => {
  // count=1, flags=0x03 (shared + has-max), min=1, max=1.
  const shared = mod(5, 4, 1, 0x03, 1, 1);
  assert.throws(() => checkMemoryLimits(shared), /shared/);
});

test("rejects an unbounded memory (no declared maximum)", () => {
  // count=1, flags=0x00 (no max), min=1.
  const unbounded = mod(5, 3, 1, 0x00, 1);
  assert.throws(() => checkMemoryLimits(unbounded), /bounded maximum/);
});

test("rejects a maximum above the 256-page (16 MiB) cap", () => {
  // count=1, flags=0x01, min=1, max=300 (LEB128: 0xAC 0x02).
  const huge = mod(5, 5, 1, 0x01, 1, 0xac, 0x02);
  assert.throws(() => checkMemoryLimits(huge), /cap/);
});

test("accepts a single bounded memory within the cap", () => {
  // count=1, flags=0x01, min=1, max=1.
  const ok = mod(5, 4, 1, 0x01, 1, 1);
  assert.doesNotThrow(() => checkMemoryLimits(ok));
  // And a module with no memory section at all (nothing to reject) is fine here.
  assert.doesNotThrow(() => checkMemoryLimits(mod()));
});
