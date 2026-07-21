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
  if (!Number.isInteger(ncells) || ncells < 0) {
    throw new FacetAbiError("ncells must be a non-negative integer");
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
  const out = new Uint32Array(ncells);
  if (LITTLE_ENDIAN) {
    // slice() copies bytes into a fresh, aligned buffer (no alignment constraint
    // on ptr, and detach-safe if the guest later grows memory).
    out.set(new Uint32Array(memory.buffer.slice(ptr, ptr + byteLen)));
  } else {
    const view = new DataView(memory.buffer, ptr, byteLen);
    for (let i = 0; i < ncells; i++) {
      out[i] = view.getUint32(i * TOKEN_BYTES, true);
    }
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
