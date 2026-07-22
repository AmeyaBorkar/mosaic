//! Adversarial test **Facet**: a structurally valid Facet whose `run` never
//! returns.
//!
//! It exists to prove the browser sandbox's metering-by-timeout (D9). The native
//! sandbox halts an infinite loop with fuel; the browser has no fuel, so an
//! untrusted Facet that hangs must be forcibly `terminate()`d by the Worker
//! timeout. This module exports the full Facet ABI (`memory`, `alloc`, `run`) so
//! it passes validation and genuinely reaches — and never returns from — `run`.

#![no_std]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

/// A small aligned arena so `alloc` is a real, working export (validation checks
/// that `alloc` exists; the timeout test still marshals input before calling
/// `run`). Accessed only through the host ABI, hence "dead" to the compiler.
const ARENA_LEN: usize = 64 * 1024;

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

/// Never returns. In the native sandbox fuel halts this; in the browser only the
/// Worker timeout can. The volatile read keeps the optimizer from collapsing the
/// loop into `unreachable` and eliding the divergence we are trying to test.
#[no_mangle]
pub extern "C" fn run(_in_ptr: i32, _out_ptr: i32, _ncells: i32, _stride: i32) {
    loop {
        unsafe {
            core::ptr::read_volatile(core::ptr::addr_of!(BUMP));
        }
    }
}
