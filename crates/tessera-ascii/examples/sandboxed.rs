//! The defining demo: an image rendered to ASCII by a **real, untrusted WASM
//! Facet running fully sandboxed** — purity, fuel metering, and memory bounds all
//! enforced. Flow: image → engine features → mosaic-runtime sandbox →
//! facet-ramp.wasm → glyph codepoints → composed text.
//!
//! Run from the workspace root:
//!   cargo run -p tessera-ascii --example sandboxed

use mosaic_runtime::{Limits, Sandbox};
use tessera_ascii::{Grid, ImageRef, compose_codepoints, feature};

const FACET_WASM: &[u8] = include_bytes!("../tests/facet_ramp.wasm");

fn main() {
    // A filled rectangle on black — hard edges show the Facet's directional glyphs.
    let (w, h) = (240u32, 160u32);
    let mut data = vec![0u8; w as usize * h as usize * 4];
    for y in 0..h {
        for x in 0..w {
            let inside = x > w / 4 && x < 3 * w / 4 && y > h / 4 && y < 3 * h / 4;
            let v: u8 = if inside { 255 } else { 0 };
            let i = (y as usize * w as usize + x as usize) * 4;
            data[i] = v;
            data[i + 1] = v;
            data[i + 2] = v;
            data[i + 3] = 255;
        }
    }
    let img = ImageRef::new(w, h, &data).unwrap();

    // 1) Engine (Tessera) extracts the feature buffer.
    let grid = Grid::new(w, h, 60, 2.0);
    let feats = feature::extract(&img, &grid);
    let ncells = (feats.cols * feats.rows) as usize;

    // 2) Platform runs the untrusted Facet in the sandbox over that buffer.
    let sandbox = Sandbox::new().expect("create sandbox");
    let facet = sandbox.compile(FACET_WASM).expect("compile Facet");
    let codepoints = sandbox
        .run_map(
            &facet,
            Limits::default(),
            &feats.data,
            ncells,
            feats.stride as usize,
        )
        .expect("run Facet");

    // 3) Engine composes the Facet's output tokens into text.
    let ascii = compose_codepoints(feats.cols, feats.rows, &codepoints);
    println!(
        "Rendered by a sandboxed WASM Facet ({} bytes of untrusted code):\n",
        FACET_WASM.len()
    );
    println!("{ascii}");
}
