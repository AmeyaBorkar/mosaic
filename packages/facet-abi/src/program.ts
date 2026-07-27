// DSL program helpers, browser side.
//
// A DSL Facet is bytecode with a fixed-layout header: magic[4] · stride[2] · n_params[2] ·
// n_tables[2], then one little-endian f32 per declared param. `applyParams` overwrites those
// param values — the browser mirror of `mosaic_vm::patch_params` — so a live control adjusts
// a Facet by patching, not recompiling. Changing a value never alters structure, so the
// patched program validates identically when the interpreter loads it.

import { FacetAbiError } from "./abi.ts";

/** Byte offset of the params section, immediately after the 10-byte header. */
const PARAMS_OFFSET = 10;

/** Read the little-endian `u16` declared param count from a program header. */
function paramCount(program: Uint8Array): number {
  if (program.length < PARAMS_OFFSET) {
    throw new FacetAbiError("Facet program too short to contain a header");
  }
  return new DataView(program.buffer, program.byteOffset, program.byteLength).getUint16(
    6,
    true,
  );
}

/**
 * Return a copy of `program` with its param values overwritten — the browser mirror of
 * `mosaic_vm::patch_params`. `values.length` must equal the program's declared param count.
 * This is how a live control adjusts a Facet: `applyParams`, then re-run with
 * {@link runFacetProgram} — no recompile and no server round-trip.
 */
export function applyParams(program: Uint8Array, values: number[]): Uint8Array {
  const n = paramCount(program);
  if (values.length !== n) {
    throw new FacetAbiError(
      `expected ${n} param value(s) for this Facet, got ${values.length}`,
    );
  }
  const end = PARAMS_OFFSET + n * 4;
  if (program.length < end) {
    throw new FacetAbiError("Facet program too short for its declared params");
  }
  const patched = program.slice(); // an owned, ArrayBuffer-backed copy
  const view = new DataView(patched.buffer, patched.byteOffset, patched.byteLength);
  for (let i = 0; i < n; i++) {
    view.setFloat32(PARAMS_OFFSET + i * 4, values[i]!, true); // little-endian f32
  }
  return patched;
}
