//! Bootstrap **Facet** for the ASCII Tessera: a density ramp with edge-aware
//! directional glyphs, compiled to `wasm32-unknown-unknown` and executed inside
//! `mosaic-runtime`'s sandbox.
//!
//! This is the first *real* Facet — untrusted community-style code, in a
//! high-level language, running under the same purity + fuel + memory guarantees
//! as any other. It implements the Mosaic Facet ABI by exporting `memory`,
//! `alloc`, and `run`. Per-cell features arrive as `[luminance, grad_mag,
//! grad_dir]` and each cell becomes one `u32` glyph codepoint.
//!
//! `no_std` and self-contained: no heap, no host imports, and only `core` math
//! (the angle reduction avoids `std`-only float methods).

#![no_std]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

/// A 16-byte-aligned bump arena in linear memory for host↔guest buffer exchange.
/// The bytes are accessed through raw pointers (via the host ABI), so the field
/// looks unread to the compiler.
#[allow(dead_code)]
#[repr(align(16))]
struct Arena([u8; 4 * 1024 * 1024]);
static mut ARENA: Arena = Arena([0; 4 * 1024 * 1024]);
static mut BUMP: usize = 0;

/// Reserve `size` bytes in the arena and return an 8-byte-aligned linear-memory
/// offset. A minimal bump allocator — the host calls this to place buffers.
#[no_mangle]
pub extern "C" fn alloc(size: i32) -> i32 {
    unsafe {
        let base = core::ptr::addr_of_mut!(ARENA) as usize;
        let off = (BUMP + 7) & !7;
        BUMP = off + size.max(0) as usize;
        (base + off) as i32
    }
}

const RAMP: &[u8] = b" .:-=+*#%@";
const EDGE_THRESHOLD: f32 = 0.6;

/// Map each cell's features to a glyph codepoint: an edge glyph when the gradient
/// magnitude is strong, otherwise a density-ramp glyph.
#[no_mangle]
pub extern "C" fn run(in_ptr: i32, out_ptr: i32, ncells: i32, stride: i32) {
    let ncells = ncells.max(0) as usize;
    let stride = stride.max(0) as usize;
    let inp = in_ptr as usize as *const u8;
    let out = out_ptr as usize as *mut u8;

    let mut i = 0;
    while i < ncells {
        let cell = unsafe { inp.add(i * stride * 4) };
        let luma = unsafe { core::ptr::read_unaligned(cell as *const f32) };
        let mag = if stride > 1 {
            unsafe { core::ptr::read_unaligned(cell.add(4) as *const f32) }
        } else {
            0.0
        };
        let dir = if stride > 2 {
            unsafe { core::ptr::read_unaligned(cell.add(8) as *const f32) }
        } else {
            0.0
        };

        let glyph = if mag > EDGE_THRESHOLD {
            edge_glyph(dir)
        } else {
            density_glyph(luma)
        };
        unsafe { core::ptr::write_unaligned(out.add(i * 4) as *mut u32, glyph) };
        i += 1;
    }
}

fn density_glyph(luma: f32) -> u32 {
    let l = luma.clamp(0.0, 1.0);
    let idx = (l * (RAMP.len() as f32 - 1.0) + 0.5) as usize;
    RAMP[idx.min(RAMP.len() - 1)] as u32
}

fn edge_glyph(dir: f32) -> u32 {
    let pi = core::f32::consts::PI;
    // Edge direction is perpendicular to the gradient; bias so the four bins
    // center on 0, π/4, π/2, 3π/4. Reduce into [0, π) without std float methods.
    let mut a = dir + pi / 2.0 + pi / 8.0;
    while a >= pi {
        a -= pi;
    }
    while a < 0.0 {
        a += pi;
    }
    match (a / (pi / 4.0)) as u32 {
        0 => b'-' as u32,
        1 => b'/' as u32,
        2 => b'|' as u32,
        _ => b'\\' as u32,
    }
}
