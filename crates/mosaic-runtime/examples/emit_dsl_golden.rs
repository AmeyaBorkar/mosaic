//! Emit the DSL browser-parity golden.
//!
//! Compiles a real (branchy) DSL Facet to bytecode, runs it through the sandboxed
//! interpreter Facet natively (`run_program` — the authoritative reference), and writes
//! `{program, features, ncells, stride, tokens}`. The browser host replays it via
//! `runFacetProgram` and must produce identical tokens, extending the "preview == render"
//! proof to DSL-authored Facets (audit M4). The committed `facet_interp.wasm` fixture is
//! placed by `scripts/build-facets.sh`, not here.

use mosaic_dsl::{Schema, compile};
use mosaic_runtime::{Limits, Sandbox};
use std::fs;
use std::path::PathBuf;

const FACET_INTERP: &[u8] = include_bytes!("../tests/facet_interp.wasm");

const ASCII_SCHEMA: Schema = Schema {
    stride: 8,
    features: &[
        ("luma", 0),
        ("grad_mag", 1),
        ("grad_dir", 2),
        ("u", 3),
        ("v", 4),
        ("r", 5),
        ("g", 6),
        ("b", 7),
    ],
    params: &[("threshold", 0.6)],
};

fn main() {
    // A branchy Facet exercising ternary, comparison, clamp, glyph-table, ramp, the spatial
    // slots u/v, and the colour slots r/g/b (a position- and colour-shaded density branch) —
    // so the browser-parity proof covers the new position and colour features end to end.
    let src = r#"grad_mag > threshold ? glyph(clamp(grad_dir * 1.27 + 2.0, 0, 3), "-/|\\") : ramp(clamp(luma - 0.5 * u + 0.3 * v + 0.2 * r - 0.1 * g - 0.2 * b, 0, 1), " .:-=+*#%@")"#;
    let program = compile(src, &ASCII_SCHEMA).expect("compile DSL");

    // Deterministic feature sweep (same xorshift as tests/dsl.rs) so the golden is stable.
    let mut state: u64 = 0x0DDB_1A5E_5BAD_5EED;
    let mut rng = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 40) as f32 / (1u64 << 24) as f32
    };
    let n = 64usize;
    let mut features = Vec::with_capacity(n * 8);
    for _ in 0..n {
        features.push(rng()); // luma 0..1
        features.push(rng() * 1.2); // grad_mag straddles threshold 0.6
        features.push(rng() * 6.0 - 3.0); // grad_dir
        features.push(rng()); // u 0..1
        features.push(rng()); // v 0..1
        features.push(rng()); // r 0..1
        features.push(rng()); // g 0..1
        features.push(rng()); // b 0..1
    }

    let sandbox = Sandbox::new().expect("sandbox");
    let facet = sandbox.compile(FACET_INTERP).expect("compile interp facet");
    let tokens = sandbox
        .run_program(&facet, Limits::default(), &program, &features, n, 8)
        .expect("run_program");

    let mut json = String::from("{\n");
    json.push_str(
        "  \"note\": \"DSL browser-parity golden (audit M4): runFacetProgram must match this native run_program output.\",\n",
    );
    json.push_str("  \"facetWasm\": \"fixtures/facet_interp.wasm\",\n");
    json.push_str("  \"stride\": 8,\n");
    json.push_str(&format!("  \"ncells\": {n},\n"));
    let prog: Vec<String> = program.iter().map(|b| b.to_string()).collect();
    json.push_str(&format!("  \"program\": [{}],\n", prog.join(",")));
    let feats: Vec<String> = features.iter().map(|f| format!("{f}")).collect();
    json.push_str(&format!("  \"features\": [{}],\n", feats.join(",")));
    let toks: Vec<String> = tokens.iter().map(|t| t.to_string()).collect();
    json.push_str(&format!("  \"tokens\": [{}]\n", toks.join(",")));
    json.push_str("}\n");

    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/facet-abi/test/dsl_golden.json");
    fs::write(&out, json).expect("write dsl golden");
    println!("emit_dsl_golden: wrote {} cells to {}", n, out.display());
}
