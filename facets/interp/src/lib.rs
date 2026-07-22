//! The **DSL interpreter Facet** (O3): runs a validated `mosaic-vm` bytecode program in
//! the sandbox.
//!
//! Protocol: the host writes a bytecode program into linear memory and calls
//! `load_program(ptr, len)` (returns `0` on success, `-1` if the program is rejected),
//! then calls the standard gather ABI `run(in_ptr, out_ptr, ncells, stride)`, which
//! evaluates the loaded program for every cell.
//!
//! The interpreter logic is the shared, `#![forbid(unsafe_code)]` [`mosaic_vm`] crate;
//! only the raw ABI marshalling here uses `unsafe`, exactly like every other Facet. The
//! program is untrusted and is fully re-validated inside the sandbox before it runs.

#![no_std]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

/// A 16-byte-aligned bump arena for the host-loaded program, the marshalled features, and
/// the output — within the sandbox's 16 MiB memory cap.
const ARENA_LEN: usize = 12 * 1024 * 1024;

#[allow(dead_code)]
#[repr(align(16))]
struct Arena([u8; ARENA_LEN]);
static mut ARENA: Arena = Arena([0; ARENA_LEN]);
static mut BUMP: usize = 0;

/// Location of the most-recently loaded, validated program in linear memory.
static mut PROG_PTR: usize = 0;
static mut PROG_LEN: usize = 0;
static mut PROG_OK: bool = false;

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

/// Validate and register a bytecode program written at `ptr` (`len` bytes). Returns `0` on
/// success, `-1` if `mosaic_vm::validate` rejects it — the host checks this before running,
/// so an invalid program is a clean error, not a trap.
#[no_mangle]
pub extern "C" fn load_program(ptr: i32, len: i32) -> i32 {
    let p = ptr.max(0) as usize;
    let n = len.max(0) as usize;
    let bytes = unsafe { core::slice::from_raw_parts(p as *const u8, n) };
    match mosaic_vm::validate(bytes) {
        Ok(_) => {
            unsafe {
                PROG_PTR = p;
                PROG_LEN = n;
                PROG_OK = true;
            }
            0
        }
        Err(_) => {
            unsafe {
                PROG_OK = false;
            }
            -1
        }
    }
}

/// Gather ABI: evaluate the loaded program for each of `ncells` cells — features at
/// `in_ptr` (stride `stride`), one output codepoint per cell at `out_ptr`. Traps (a clean
/// host-visible error) if no valid program is loaded or evaluation fails.
#[no_mangle]
pub extern "C" fn run(in_ptr: i32, out_ptr: i32, ncells: i32, stride: i32) {
    let ncells = ncells.max(0) as usize;
    let stride = stride.max(0) as usize;

    let (prog_ptr, prog_len, ok) = unsafe { (PROG_PTR, PROG_LEN, PROG_OK) };
    if !ok {
        core::arch::wasm32::unreachable()
    }
    let prog_bytes = unsafe { core::slice::from_raw_parts(prog_ptr as *const u8, prog_len) };
    let program = match mosaic_vm::validate(prog_bytes) {
        Ok(p) => p,
        Err(_) => core::arch::wasm32::unreachable(),
    };

    let features = unsafe {
        core::slice::from_raw_parts(in_ptr as usize as *const f32, ncells.saturating_mul(stride))
    };
    let out = unsafe { core::slice::from_raw_parts_mut(out_ptr as usize as *mut u32, ncells) };

    if mosaic_vm::run(&program, features, ncells, stride, out).is_err() {
        core::arch::wasm32::unreachable()
    }
}
