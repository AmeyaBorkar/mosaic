// The synchronous, environment-agnostic Facet marshaller.
//
// `runFacetMap` is the browser-side mirror of `mosaic-runtime::Sandbox::run_map`:
// identical stride/length/overflow checks, the same allocate-through-the-guest
// marshalling, little-endian `f32` in and `u32` tokens out. It is deliberately a
// pure synchronous function with zero dependence on Worker/timeout machinery, so
// it can be conformance-tested directly against a golden vector emitted by the
// (proven) native host. The untrusted-execution policy (Worker + timeout) lives
// in `sandbox.ts` and calls into this.

import {
  FEATURE_BYTES,
  TOKEN_BYTES,
  FacetAbiError,
  checkedWasmByteLen,
} from "./abi.ts";

/** Whether this platform lays out multi-byte integers little-endian. */
const LITTLE_ENDIAN = new Uint8Array(new Uint32Array([1]).buffer)[0] === 1;

/** Maximum linear-memory pages an untrusted Facet may declare (16 MiB / 64 KiB) —
 *  the browser analogue of the native StoreLimits memory cap. */
const MAX_MEMORY_PAGES = 256;

/** Read an unsigned LEB128 integer at `off`; returns `[value, nextOffset]`. */
function readUleb(bytes: Uint8Array, off: number): [number, number] {
  let result = 0;
  let shift = 0;
  let pos = off;
  for (let i = 0; i < 5; i++) {
    const byte = bytes[pos++] ?? 0;
    result |= (byte & 0x7f) << shift;
    if ((byte & 0x80) === 0) break;
    shift += 7;
  }
  return [result >>> 0, pos];
}

/** Declared limits of each linear memory *defined* in the module (imported memory,
 *  if any, is already rejected as an import). Parsed from the module bytes because
 *  `WebAssembly.Module` reflection exposes export kinds but not memory limits. */
function readMemoryLimits(
  bytes: Uint8Array,
): Array<{ min: number; max: number | undefined; shared: boolean }> {
  const out: Array<{ min: number; max: number | undefined; shared: boolean }> = [];
  let off = 8; // skip the 8-byte magic + version (already validated by compile)
  while (off < bytes.length) {
    const id = bytes[off++] ?? 0;
    const [size, afterSize] = readUleb(bytes, off);
    off = afterSize;
    const sectionEnd = off + size;
    if (id === 5) {
      let p = off;
      const [count, afterCount] = readUleb(bytes, p);
      p = afterCount;
      for (let i = 0; i < count; i++) {
        const flags = bytes[p++] ?? 0;
        const [min, afterMin] = readUleb(bytes, p);
        p = afterMin;
        let max: number | undefined;
        if ((flags & 0x01) !== 0) {
          const [m, afterMax] = readUleb(bytes, p);
          p = afterMax;
          max = m;
        }
        out.push({ min, max, shared: (flags & 0x02) !== 0 });
      }
    }
    off = sectionEnd;
  }
  return out;
}

/**
 * Reject a Facet whose defined linear memory is unbounded, shared, or declares a
 * maximum above the cap. The browser enforces a declared maximum on `memory.grow`,
 * so bounding it here contains a memory-bomb Facet the way the native StoreLimits
 * cap does — growth past the cap traps rather than committing gigabytes into the
 * page. Called on the (already-compiled, valid) module bytes.
 */
export function checkMemoryLimits(bytes: BufferSource): void {
  const u8 = ArrayBuffer.isView(bytes)
    ? new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength)
    : new Uint8Array(bytes);
  for (const mem of readMemoryLimits(u8)) {
    if (mem.shared) {
      throw new FacetAbiError("Facet memory must not be shared");
    }
    if (mem.max === undefined) {
      throw new FacetAbiError("Facet memory must declare a bounded maximum");
    }
    if (mem.max > MAX_MEMORY_PAGES) {
      throw new FacetAbiError(
        `Facet memory maximum of ${mem.max} pages exceeds the ${MAX_MEMORY_PAGES}-page (16 MiB) cap`,
      );
    }
  }
}

/** Required exports and their kinds, matching what `run_map` looks up. */
const REQUIRED_EXPORTS: ReadonlyArray<readonly [string, WebAssembly.ImportExportKind]> = [
  ["memory", "memory"],
  ["alloc", "function"],
  ["run", "function"],
];

/**
 * Validate a Facet module structurally, reproducing wasmtime's guarantees:
 * **zero imports** (purity is granted no ambient authority) and the required ABI
 * exports. A module that declares *any* import is rejected before it can run,
 * exactly as the native host grants no imports at instantiation.
 */
export function validateFacetModule(module: WebAssembly.Module): void {
  const imports = WebAssembly.Module.imports(module);
  if (imports.length > 0) {
    const names = imports.map((i) => `${i.module}.${i.name}`).join(", ");
    throw new FacetAbiError(
      `Facet must declare zero imports (purity); found: ${names}`,
    );
  }
  const exports = new Map(
    WebAssembly.Module.exports(module).map((e) => [e.name, e.kind] as const),
  );
  for (const [name, kind] of REQUIRED_EXPORTS) {
    const got = exports.get(name);
    if (got === undefined) {
      throw new FacetAbiError(`Facet does not export '${name}'`);
    }
    if (got !== kind) {
      throw new FacetAbiError(
        `Facet export '${name}' must be a ${kind}, found ${got}`,
      );
    }
  }
}

/** Compile untrusted Facet bytes and validate them. Async: compilation is the
 *  one step browsers stream off the main thread. */
export async function compileFacet(bytes: BufferSource): Promise<WebAssembly.Module> {
  let module: WebAssembly.Module;
  try {
    module = await WebAssembly.compile(bytes);
  } catch (e) {
    throw new FacetAbiError(`Facet failed to compile: ${messageOf(e)}`);
  }
  checkMemoryLimits(bytes);
  validateFacetModule(module);
  return module;
}

interface FacetExports {
  memory: WebAssembly.Memory;
  alloc: (size: number) => number;
  run: (inPtr: number, outPtr: number, ncells: number, stride: number) => void;
}

/**
 * Run a validated Facet over a feature buffer, returning one `u32` token per
 * cell. Synchronous and pure. Every crossing is bounds-checked against the
 * guest's live memory, so a Facet returning a bogus pointer yields a
 * `FacetAbiError`, never a corrupt read/write — the browser analogue of
 * wasmtime's checked `memory.read/write`.
 *
 * Preconditions mirror `run_map`: `stride > 0`, and `features.length` must equal
 * `ncells * stride`.
 */
export function runFacetMap(
  module: WebAssembly.Module,
  features: Float32Array,
  ncells: number,
  stride: number,
): Uint32Array {
  if (!Number.isInteger(stride) || stride <= 0) {
    throw new FacetAbiError("stride must be a positive integer");
  }
  if (stride > 0x7fff_ffff) {
    throw new FacetAbiError("stride exceeds the i32 range");
  }
  if (!Number.isInteger(ncells) || ncells < 0) {
    throw new FacetAbiError("ncells must be a non-negative integer");
  }
  if (ncells > 0x7fff_ffff) {
    throw new FacetAbiError("ncells exceeds the i32 range");
  }
  const expected = ncells * stride;
  if (features.length !== expected) {
    throw new FacetAbiError(
      `features length ${features.length} != ncells * stride (${expected})`,
    );
  }

  // Size every buffer up front, rejecting overflow / out-of-range before any
  // guest allocation — an untrusted size can never drive a host allocation.
  const inByteLen = checkedWasmByteLen(features.length, FEATURE_BYTES);
  const outByteLen = checkedWasmByteLen(ncells, TOKEN_BYTES);

  // Zero imports: the guest receives no host functions, memories, or globals.
  let instance: WebAssembly.Instance;
  try {
    instance = new WebAssembly.Instance(module, {});
  } catch (e) {
    throw new FacetAbiError(`Facet failed to instantiate: ${messageOf(e)}`);
  }
  const exports = instance.exports as Partial<FacetExports>;
  const { memory, alloc, run } = exports;
  if (
    !(memory instanceof WebAssembly.Memory) ||
    typeof alloc !== "function" ||
    typeof run !== "function"
  ) {
    throw new FacetAbiError("Facet instance is missing required exports");
  }
  // Mirror the native get_typed_func signature check: alloc(i32)->i32 has arity 1,
  // run(i32,i32,i32,i32)->() has arity 4. A wrong-signature export is rejected here
  // instead of silently producing garbage (extra args dropped / missing zero-filled).
  if (alloc.length !== 1 || run.length !== 4) {
    throw new FacetAbiError(
      `Facet export arity is wrong: alloc/${alloc.length} run/${run.length} (expected alloc/1 run/4)`,
    );
  }

  const inPtr = callGuest("alloc", () => alloc(inByteLen));
  writeFeaturesLE(memory, inPtr, features);
  const outPtr = callGuest("alloc", () => alloc(outByteLen));

  // Reserve the output region defensively before running, so a Facet that writes
  // fewer cells than promised still yields a well-defined (zero-filled) buffer,
  // and one that would write past the end is caught here rather than corrupting.
  ensureRange(memory, outPtr, outByteLen);

  callGuest("run", () => run(inPtr, outPtr, ncells, stride));

  return readTokensLE(memory, outPtr, ncells);
}

/** Write `features` as little-endian `f32` at `ptr`, bounds-checked. */
function writeFeaturesLE(
  memory: WebAssembly.Memory,
  ptr: number,
  features: Float32Array,
): void {
  const byteLen = features.length * FEATURE_BYTES;
  ensureRange(memory, ptr, byteLen);
  if (LITTLE_ENDIAN) {
    const src = new Uint8Array(features.buffer, features.byteOffset, byteLen);
    new Uint8Array(memory.buffer, ptr, byteLen).set(src);
  } else {
    const view = new DataView(memory.buffer, ptr, byteLen);
    for (let i = 0; i < features.length; i++) {
      view.setFloat32(i * FEATURE_BYTES, features[i]!, true);
    }
  }
}

/** Read `ncells` little-endian `u32` tokens at `ptr`, bounds-checked. */
function readTokensLE(
  memory: WebAssembly.Memory,
  ptr: number,
  ncells: number,
): Uint32Array {
  const byteLen = ncells * TOKEN_BYTES;
  ensureRange(memory, ptr, byteLen);
  if (LITTLE_ENDIAN) {
    // slice() copies into a fresh, aligned, detach-safe buffer — a single allocation
    // and a single copy (no separately zero-filled destination).
    return new Uint32Array(memory.buffer.slice(ptr, ptr + byteLen));
  }
  const out = new Uint32Array(ncells);
  const view = new DataView(memory.buffer, ptr, byteLen);
  for (let i = 0; i < ncells; i++) {
    out[i] = view.getUint32(i * TOKEN_BYTES, true);
  }
  return out;
}

/**
 * Assert `[ptr, ptr + len)` lies within the guest's current linear memory. A
 * malicious `alloc` can return any `i32`; wasm returns it to JS as a signed
 * number, so a high-bit address arrives negative and is rejected here.
 */
function ensureRange(memory: WebAssembly.Memory, ptr: number, len: number): void {
  if (!Number.isInteger(ptr) || ptr < 0) {
    throw new FacetAbiError(`Facet returned an invalid pointer (${ptr})`);
  }
  const end = ptr + len;
  if (end > memory.buffer.byteLength) {
    throw new FacetAbiError(
      `Facet access out of bounds: [${ptr}, ${end}) exceeds memory of ${memory.buffer.byteLength} bytes`,
    );
  }
}

/** Invoke a guest export, converting a trap into a `FacetAbiError`. */
function callGuest<T>(what: string, fn: () => T): T {
  try {
    return fn();
  } catch (e) {
    throw new FacetAbiError(`Facet trapped in '${what}': ${messageOf(e)}`);
  }
}

function messageOf(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
