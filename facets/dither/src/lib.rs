//! **Propagation** Facet (decision D5): 1-bit Floyd–Steinberg error-diffusion
//! dithering.
//!
//! Exports the 2-D ABI `run2d(in_ptr, out_ptr, cols, rows, stride)`. It builds a
//! mutable slice over the host-marshalled feature buffer and the output buffer and
//! hands them to the shared [`dither::floyd_steinberg`]. Because that is the exact
//! routine the native engine runs, the sandboxed preview is bit-identical to the
//! render — one implementation, compiled twice.
//!
//! `no_std`, no host imports, only `f32` arithmetic — pure and deterministic.

#![no_std]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

/// Arena sized to hold a render's marshalled input plus output up to the engine's
/// `MAX_FEATURE_BYTES` (8 MiB) budget, within the sandbox's 16 MiB memory cap.
const ARENA_LEN: usize = 12 * 1024 * 1024;

#[allow(dead_code)]
#[repr(align(16))]
struct Arena([u8; ARENA_LEN]);
static mut ARENA: Arena = Arena([0; ARENA_LEN]);
static mut BUMP: usize = 0;

#[no_mangle]
pub extern "C" fn alloc(size: i32) -> i32 {
    unsafe {
        let base = core::ptr::addr_of_mut!(ARENA) as usize;
        let off = (BUMP + 7) & !7;
        let end = off.saturating_add(size.max(0) as usize);
        if end > ARENA_LEN {
            return -1;
        }
        BUMP = end;
        (base + off) as i32
    }
}

/// Error-diffusion dither the per-cell luminance (slot 0 of each cell's `stride`
/// features) into one output codepoint per cell, via the shared routine. The host
/// wrote `cols*rows*stride` little-endian `f32` at `in_ptr` (8-byte aligned by
/// `alloc`) and allocated `cols*rows` `u32` at `out_ptr`.
#[no_mangle]
pub extern "C" fn run2d(in_ptr: i32, out_ptr: i32, cols: i32, rows: i32, stride: i32) {
    let cols = cols.max(0) as usize;
    let rows = rows.max(0) as usize;
    let stride = stride.max(0) as usize;
    let ncells = cols.saturating_mul(rows);

    let features = unsafe {
        core::slice::from_raw_parts_mut(
            in_ptr as usize as *mut f32,
            ncells.saturating_mul(stride),
        )
    };
    let out = unsafe { core::slice::from_raw_parts_mut(out_ptr as usize as *mut u32, ncells) };

    dither::floyd_steinberg(features, cols, rows, stride, out);
}
