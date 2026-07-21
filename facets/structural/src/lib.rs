//! **L2 structural** Facet: matches each cell's sub-cell luminance patch to the
//! closest glyph in the shared [`glyph_atlas`].
//!
//! The engine extracts the patch (stride = `PATCH_SLOTS` little-endian `f32` per
//! cell); this Facet only *classifies* it. Because it calls the exact same
//! `glyph_atlas::match_glyph` the native engine calls, the untrusted preview cannot
//! diverge from the authoritative render — there is one matcher, compiled twice.
//!
//! `no_std`, no host imports, only `f32` arithmetic — pure, deterministic, and
//! bit-identical to the native path.

#![no_std]

use core::panic::PanicInfo;
use glyph_atlas::{match_glyph, PATCH_SLOTS};

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

/// A 16-byte-aligned bump arena for host↔guest buffer exchange. Sized for the L2
/// stride (64 `f32`/cell): 8 MiB holds ~30k cells of input plus output, well within
/// the sandbox's memory cap.
#[allow(dead_code)]
#[repr(align(16))]
struct Arena([u8; 8 * 1024 * 1024]);
static mut ARENA: Arena = Arena([0; 8 * 1024 * 1024]);
static mut BUMP: usize = 0;

#[no_mangle]
pub extern "C" fn alloc(size: i32) -> i32 {
    unsafe {
        let base = core::ptr::addr_of_mut!(ARENA) as usize;
        let off = (BUMP + 7) & !7;
        BUMP = off + size.max(0) as usize;
        (base + off) as i32
    }
}

/// For each cell, read its `PATCH_SLOTS` sub-cell luminance samples and emit the
/// codepoint of the closest atlas glyph.
#[no_mangle]
pub extern "C" fn run(in_ptr: i32, out_ptr: i32, ncells: i32, stride: i32) {
    let ncells = ncells.max(0) as usize;
    let stride = stride.max(0) as usize;
    let inp = in_ptr as usize as *const u8;
    let out = out_ptr as usize as *mut u8;

    let mut patch = [0.0f32; PATCH_SLOTS];
    let mut i = 0;
    while i < ncells {
        let cell = unsafe { inp.add(i * stride * 4) };
        let n = if stride < PATCH_SLOTS {
            stride
        } else {
            PATCH_SLOTS
        };
        let mut k = 0;
        while k < n {
            patch[k] = unsafe { core::ptr::read_unaligned(cell.add(k * 4) as *const f32) };
            k += 1;
        }
        // Clear any unread tail (buffer is reused across cells).
        while k < PATCH_SLOTS {
            patch[k] = 0.0;
            k += 1;
        }
        let glyph = match_glyph(&patch);
        unsafe { core::ptr::write_unaligned(out.add(i * 4) as *mut u32, glyph) };
        i += 1;
    }
}
