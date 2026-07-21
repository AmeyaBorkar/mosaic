//! Adversarial test **Facet**: structurally valid (exports `memory`, `alloc`,
//! `run`) but its `alloc` returns a wild pointer (`-1`). It proves the host
//! rejects a bogus guest pointer with a clean error instead of reading or writing
//! out of bounds — the browser analogue of the native
//! `map_rejects_wild_alloc_pointer` test.

#![no_std]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

/// A small static so the module has linear memory to export; touched via a
/// volatile read in `run` so the optimizer keeps it (and the memory) alive.
#[allow(dead_code)]
static mut SINK: [u8; 64] = [0; 64];

/// Returns a wild pointer regardless of `size`. As an `i32` this is `-1`, which
/// the host sees as a negative address and must reject before any access.
#[no_mangle]
pub extern "C" fn alloc(_size: i32) -> i32 {
    -1
}

/// No-op; unreached in the bounds test because marshalling the input fails first.
#[no_mangle]
pub extern "C" fn run(_in_ptr: i32, _out_ptr: i32, _ncells: i32, _stride: i32) {
    unsafe {
        core::ptr::read_volatile(core::ptr::addr_of!(SINK) as *const u8);
    }
}
