//! End-to-end O3 (phase A): the **DSL interpreter Facet** runs a validated `mosaic-vm`
//! bytecode program in the sandbox (`run_program`) and produces output byte-identical to
//! the **native** `mosaic-vm` reference — proving the untrusted, sandboxed VM matches the
//! host VM (they are literally the same crate, compiled twice). Also checks that a
//! malformed program is rejected as a clean error, never a panic.
//!
//! The fixture is built from `facets/interp`:
//!   RUSTFLAGS="-C link-arg=--max-memory=16777216" \
//!     cargo build --manifest-path facets/interp/Cargo.toml --target wasm32-unknown-unknown --release
//! and copied next to this file.

use mosaic_runtime::{Limits, Sandbox};
use mosaic_vm::{MAGIC, op};

const FACET_INTERP: &[u8] = include_bytes!("facet_interp.wasm");

/// Serialize a program from its parts (the compiler in `mosaic-dsl` does this for real).
fn program(stride: u16, params: &[f32], tables: &[&[u32]], code: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&MAGIC.to_le_bytes());
    b.extend_from_slice(&stride.to_le_bytes());
    b.extend_from_slice(&(params.len() as u16).to_le_bytes());
    b.extend_from_slice(&(tables.len() as u16).to_le_bytes());
    for &p in params {
        b.extend_from_slice(&p.to_bits().to_le_bytes());
    }
    for t in tables {
        b.extend_from_slice(&(t.len() as u16).to_le_bytes());
        for &c in *t {
            b.extend_from_slice(&c.to_le_bytes());
        }
    }
    b.extend_from_slice(&(code.len() as u32).to_le_bytes());
    b.extend_from_slice(code);
    b
}

fn konst(code: &mut Vec<u8>, v: f32) {
    code.push(op::CONST);
    code.extend_from_slice(&v.to_bits().to_le_bytes());
}
fn loadf(code: &mut Vec<u8>, slot: u16) {
    code.push(op::LOADF);
    code.extend_from_slice(&slot.to_le_bytes());
}

const RAMP: &[u32] = &[0x20, 0x2E, 0x3A, 0x2D, 0x3D, 0x2B, 0x2A, 0x23, 0x25, 0x40]; // " .:-=+*#%@"

/// idx = floor(clamp(luma,0,1) * 9 + 0.5); ramp[idx] — the density method.
fn density_program() -> Vec<u8> {
    let mut code = Vec::new();
    loadf(&mut code, 0); // luma (slot 0 of a stride-3 buffer)
    konst(&mut code, 0.0);
    konst(&mut code, 1.0);
    code.push(op::CLAMP);
    konst(&mut code, (RAMP.len() - 1) as f32);
    code.push(op::MUL);
    konst(&mut code, 0.5);
    code.push(op::ADD);
    code.push(op::FLOOR);
    code.push(op::TABLE);
    code.extend_from_slice(&0u16.to_le_bytes());
    code.push(op::END);
    program(3, &[], &[RAMP], &code)
}

#[test]
fn sandboxed_interpreter_matches_native_reference() {
    let prog_bytes = density_program();

    // Native reference.
    let n = 200usize;
    let mut features = Vec::with_capacity(n * 3);
    for i in 0..n {
        features.push(i as f32 / (n - 1) as f32); // luma
        features.push(0.0);
        features.push(0.0);
    }
    let validated = mosaic_vm::validate(&prog_bytes).unwrap();
    let mut native = vec![0u32; n];
    mosaic_vm::run(&validated, &features, n, 3, &mut native).unwrap();

    // Sandboxed interpreter.
    let sandbox = Sandbox::new().unwrap();
    let facet = sandbox.compile(FACET_INTERP).unwrap();
    let sandboxed = sandbox
        .run_program(&facet, Limits::default(), &prog_bytes, &features, n, 3)
        .unwrap();

    assert_eq!(
        sandboxed, native,
        "sandboxed interpreter diverged from the native mosaic-vm reference"
    );
}

#[test]
fn sandboxed_branching_program() {
    // if f0 > 0.5 { '@' } else { '.' } — proves SELECT works through the sandbox.
    let mut code = Vec::new();
    loadf(&mut code, 0);
    konst(&mut code, 0.5);
    code.push(op::GT); // cond
    konst(&mut code, 0x40 as f32); // '@'
    konst(&mut code, 0x2E as f32); // '.'
    code.push(op::SELECT);
    code.push(op::END);
    let prog_bytes = program(1, &[], &[], &code);

    let sandbox = Sandbox::new().unwrap();
    let facet = sandbox.compile(FACET_INTERP).unwrap();
    let features = [0.9f32, 0.1, 0.7, 0.2];
    let out = sandbox
        .run_program(&facet, Limits::default(), &prog_bytes, &features, 4, 1)
        .unwrap();
    assert_eq!(out, vec![0x40, 0x2E, 0x40, 0x2E]);
}

#[test]
fn malformed_program_is_a_clean_error_not_a_panic() {
    let sandbox = Sandbox::new().unwrap();
    let facet = sandbox.compile(FACET_INTERP).unwrap();
    // Bad magic: load_program returns -1, so run_program errors before running.
    let bad = vec![0u8; 8];
    let result = sandbox.run_program(&facet, Limits::default(), &bad, &[0.0, 0.0, 0.0], 1, 3);
    assert!(result.is_err(), "a rejected program must surface as an Err");
}
