//! The full O3 loop, proven end to end: an author writes a Facet in the **DSL text**, it
//! compiles to bytecode (`mosaic-dsl`), and runs **untrusted in the sandbox** via the
//! interpreter Facet (`run_program`) — producing exactly what the reference produces.
//!
//! - A DSL `ramp(luma, ...)` Facet renders density byte-identical to the native reference
//!   mapping (the same glyphs `facet-ramp` produces for the density path).
//! - A richer density-or-edge DSL Facet runs in the sandbox byte-identical to the native
//!   `mosaic-vm`, is non-degenerate, and is deterministic.

use mosaic_dsl::{Schema, compile};
use mosaic_runtime::{Limits, Sandbox};

const FACET_INTERP: &[u8] = include_bytes!("facet_interp.wasm");
const RAMP: &str = " .:-=+*#%@";

const ASCII_SCHEMA: Schema = Schema {
    stride: 3,
    features: &[("luma", 0), ("grad_mag", 1), ("grad_dir", 2)],
    params: &[("threshold", 0.6)],
};

fn native_density(luma: f32) -> u32 {
    let l = luma.clamp(0.0, 1.0);
    let n = RAMP.chars().count();
    let idx = (l * (n as f32 - 1.0) + 0.5) as usize;
    RAMP.chars().nth(idx.min(n - 1)).unwrap() as u32
}

#[test]
fn dsl_density_facet_renders_in_sandbox_matching_native() {
    let bytes = compile(r#"ramp(luma, " .:-=+*#%@")"#, &ASCII_SCHEMA).unwrap();

    let n = 200usize;
    let mut features = Vec::with_capacity(n * 3);
    for i in 0..n {
        features.push(i as f32 / (n - 1) as f32);
        features.push(0.0);
        features.push(0.0);
    }

    let sandbox = Sandbox::new().unwrap();
    let facet = sandbox.compile(FACET_INTERP).unwrap();
    let out = sandbox
        .run_program(&facet, Limits::default(), &bytes, &features, n, 3)
        .unwrap();

    for (i, &tok) in out.iter().enumerate() {
        assert_eq!(
            tok,
            native_density(i as f32 / (n - 1) as f32),
            "DSL Facet diverged from native density at cell {i}"
        );
    }
}

#[test]
fn dsl_edge_or_density_facet_sandbox_equals_reference() {
    // A real, branchy Facet: strong edges pick a directional glyph, else density.
    let src = r#"grad_mag > threshold ? glyph(clamp(grad_dir * 1.27 + 2.0, 0, 3), "-/|\\") : ramp(luma, " .:-=+*#%@")"#;
    let bytes = compile(src, &ASCII_SCHEMA).unwrap();

    // Deterministic feature sweep with a mix of weak and strong "edges".
    let mut state: u64 = 0x0DDB_1A5E_5BAD_5EED;
    let mut rng = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 40) as f32 / (1u64 << 24) as f32
    };
    let n = 300usize;
    let mut features = Vec::with_capacity(n * 3);
    for _ in 0..n {
        features.push(rng()); // luma 0..1
        features.push(rng() * 1.2); // grad_mag 0..1.2 (straddles threshold 0.6)
        features.push(rng() * 6.0 - 3.0); // grad_dir, a varied angle range
    }

    // Native reference.
    let validated = mosaic_vm::validate(&bytes).unwrap();
    let mut native = vec![0u32; n];
    mosaic_vm::run(&validated, &features, n, 3, &mut native).unwrap();

    // Sandboxed.
    let sandbox = Sandbox::new().unwrap();
    let facet = sandbox.compile(FACET_INTERP).unwrap();
    let sandboxed = sandbox
        .run_program(&facet, Limits::default(), &bytes, &features, n, 3)
        .unwrap();

    assert_eq!(
        sandboxed, native,
        "sandboxed DSL Facet diverged from the native reference"
    );
    // Non-degenerate: both branches fire, so more than one glyph appears.
    let distinct: std::collections::BTreeSet<u32> = sandboxed.iter().copied().collect();
    assert!(distinct.len() > 1, "expected a non-degenerate render");
    // Deterministic re-run.
    let again = sandbox
        .run_program(&facet, Limits::default(), &bytes, &features, n, 3)
        .unwrap();
    assert_eq!(again, sandboxed);
}
