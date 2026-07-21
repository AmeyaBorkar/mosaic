// The Mosaic Facet ABI (see docs/architecture.md D8), browser side.
//
// A Facet exports `memory`, `alloc(i32) -> i32`, and
// `run(in_ptr, out_ptr, ncells, stride)`. The host allocates through the guest's
// own allocator, writes a per-cell feature buffer as little-endian `f32`, calls
// `run`, and reads back one little-endian `u32` output token per cell. This file
// holds the constants and the shared error type; `host.ts` implements the
// marshalling that must match `mosaic-runtime::Sandbox::run_map` byte-for-byte.

/** Size of one feature value in the input buffer: a little-endian `f32`. */
export const FEATURE_BYTES = 4;

/** Size of one output token: a little-endian `u32` (for ASCII, a codepoint). */
export const TOKEN_BYTES = 4;

/**
 * Largest byte length addressable in 32-bit wasm. Mirrors the native host's
 * `i32::MAX` cap in `checked_wasm_len`, so an untrusted size can never drive an
 * out-of-range host allocation — it becomes a clean error instead.
 */
export const MAX_WASM_LEN = 0x7fff_ffff; // 2_147_483_647

/** Every failure at the Facet boundary surfaces as this single error type. */
export class FacetAbiError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "FacetAbiError";
  }
}

/**
 * Compute `count * elemSize` as a byte length usable as a 32-bit wasm address,
 * rejecting non-integers, negatives, overflow, or anything past `i32::MAX`.
 * The direct analogue of the native `checked_wasm_len`.
 */
export function checkedWasmByteLen(count: number, elemSize: number): number {
  if (!Number.isInteger(count) || count < 0) {
    throw new FacetAbiError(`invalid element count ${count}`);
  }
  // count and elemSize are non-negative integers < 2^53, so the product is exact.
  const bytes = count * elemSize;
  if (bytes > MAX_WASM_LEN) {
    throw new FacetAbiError(
      `buffer length ${bytes} exceeds 32-bit wasm addressing (${MAX_WASM_LEN})`,
    );
  }
  return bytes;
}
