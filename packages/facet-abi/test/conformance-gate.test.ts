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
import { checkMemoryLimits, checkTableLimits, compileFacet } from "../src/host.ts";

const MAGIC = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]; // "\0asm" + version 1

/** A minimal module: the 8-byte header followed by a raw section (id, size, body…). */
function mod(...section: number[]): Uint8Array<ArrayBuffer> {
  return new Uint8Array([...MAGIC, ...section]);
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

test("rejects a 64-bit (memory64) memory (native disables memory64)", () => {
  // count=1, flags=0x05 (has-max + is64 bit), min=1, max=1. wasmtime enables memory64
  // by default via WASM3, so the browser must reject it or it previews what the server
  // refuses.
  const mem64 = mod(5, 4, 1, 0x05, 1, 1);
  assert.throws(() => checkMemoryLimits(mem64), /64-bit|memory64/);
});

test("rejects an overflowing LEB128 maximum instead of truncating it", () => {
  // max = LEB128 0x80 0x82 0x80 0x80 0x10 = 4_294_967_552 (> u32). The old 32-bit
  // `<< 28` wrapped this to a small in-cap value; it must now be rejected.
  // count=1, flags=0x01, min=1, then the 5-byte max (body = 8 bytes).
  const overflow = mod(5, 8, 1, 0x01, 1, 0x80, 0x82, 0x80, 0x80, 0x10);
  assert.throws(() => checkMemoryLimits(overflow), /exceeds u32|LEB128/);
});

// Table section (id 4): [id, size, count, entries…]; entry = [reftype, flags, min, (max)?].
// reftype 0x70 = funcref. Native caps tables at 1 table / 10 000 elements.

test("rejects a table larger than the 10,000-element cap", () => {
  // count=1; funcref; flags=0x00 (no max); min=20000 (LEB 0xA0 0x9C 0x01). body = 6.
  const bigTable = mod(4, 6, 1, 0x70, 0x00, 0xa0, 0x9c, 0x01);
  assert.throws(() => checkTableLimits(bigTable), /element cap/);
});

test("rejects more than one table (native StoreLimits caps at one)", () => {
  // count=2, two funcref [1] tables (entry = reftype, flags 0x00, min 1). body = 7.
  const twoTables = mod(4, 7, 2, 0x70, 0x00, 1, 0x70, 0x00, 1);
  assert.throws(() => checkTableLimits(twoTables), /at most one table/);
});

test("accepts a single small table, and no table section", () => {
  // count=1, funcref, flags=0x01 (has-max), min=1, max=1. body = 5.
  const ok = mod(4, 5, 1, 0x70, 0x01, 1, 1);
  assert.doesNotThrow(() => checkTableLimits(ok));
  assert.doesNotThrow(() => checkTableLimits(mod()));
});

test("rejects a module larger than the 8 MiB cap before compiling", async () => {
  // 8 MiB + 1 byte of a valid header + padding; the length check fires before compile.
  const oversized = new Uint8Array(8 * 1024 * 1024 + 1);
  oversized.set([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]);
  await assert.rejects(() => compileFacet(oversized), /exceeding the .* limit/);
});

test("finds an over-cap memory section behind a preceding custom section", () => {
  // Non-vacuous: a broken section walk that assumed the memory section sits right after
  // the header would read the *custom* section as memory and miss this. The parser must
  // skip the custom section (id 0, size 3: name "ab") to reach the over-cap memory
  // section (max=300 > 256). Audit M13.
  const custom = [0, 3, 2, 0x61, 0x62]; // id 0, size 3, namelen 2, 'a','b'
  const overCapMem = [5, 5, 1, 0x01, 1, 0xac, 0x02]; // max=300 (LEB 0xAC 0x02)
  const withCustom = mod(...custom, ...overCapMem);
  assert.throws(() => checkMemoryLimits(withCustom), /cap/);
});
