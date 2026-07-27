//! End-to-end DSL pixel-art demo: author a Facet in the **DSL text**, compile it to
//! bytecode, and run it **untrusted in the sandbox** over a real image's per-cell
//! luminance — producing block "pixels". Ties together the community authoring layer
//! (DSL), the safe runtime (sandbox), and the output primitive (glyph grid).
//!
//! The luminance grid is produced out-of-band (any PNG decoder); this program takes it as
//! a whitespace-separated list of `cols*rows` values in [0,1], row-major.
//!
//! Run: `cargo run -p mosaic-runtime --example dsl_pixel_art -- <luma.txt> <cols> <rows>`

use std::{env, fs};

use mosaic_dsl::{Schema, compile};
use mosaic_runtime::{Limits, Sandbox};

const FACET_INTERP: &[u8] = include_bytes!("../tests/facet_interp.wasm");

fn main() {
    let args: Vec<String> = env::args().collect();
    let luma_path = &args[1];
    let cols: usize = args[2].parse().expect("cols");
    let rows: usize = args[3].parse().expect("rows");
    let ncells = cols * rows;

    let features: Vec<f32> = fs::read_to_string(luma_path)
        .expect("read luma")
        .split_whitespace()
        .map(|s| s.parse().expect("f32"))
        .collect();
    assert_eq!(features.len(), ncells, "luma count != cols*rows");

    // 1) The Facet, authored in the DSL: map luminance to a Unicode block ramp.
    let schema = Schema {
        stride: 1,
        features: &[("luma", 0)],
        params: &[],
    };
    // The DSL source is arg 4 if given, else a plain block ramp. Note it can do more
    // than map: `clamp(luma * 2.6, 0, 1)` boosts contrast before the ramp, all in-Facet.
    let default_src = r#"ramp(luma, " .:░▒▓█")"#.to_string();
    let src = args.get(4).cloned().unwrap_or(default_src);
    let program = compile(&src, &schema).expect("compile DSL");

    // 2) Run it untrusted in the sandbox (zero imports, fuel + memory + epoch bounded).
    let sandbox = Sandbox::new().expect("sandbox");
    let facet = sandbox
        .compile(FACET_INTERP)
        .expect("compile interpreter Facet");
    let tokens = sandbox
        .run_program(&facet, Limits::default(), &program, &features, ncells, 1)
        .expect("run_program");

    // 3) Compose the output tokens (codepoints) into text.
    let mut out = String::with_capacity(ncells + rows);
    for r in 0..rows {
        for c in 0..cols {
            out.push(char::from_u32(tokens[r * cols + c]).unwrap_or('\u{FFFD}'));
        }
        out.push('\n');
    }

    eprintln!(
        "DSL: {src}\ncompiled to {} bytes of bytecode, run in the sandbox over {ncells} cells ({cols}x{rows}):\n",
        program.len()
    );
    print!("{out}");
}
