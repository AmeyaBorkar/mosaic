//! End-to-end proof for **L2**: the real `facet-structural` WASM module, run inside
//! `mosaic-runtime`'s sandbox, produces byte-identical ASCII to the native
//! `render_structural` path. Both sides call the *same* `glyph_atlas::match_glyph`,
//! so this is also the guarantee that the untrusted preview equals the render.
//!
//! The fixture `facet_structural.wasm` is generated from `facets/structural`; rebuild:
//!   cargo build --manifest-path facets/structural/Cargo.toml --target wasm32-unknown-unknown --release
//! and copy it next to this file.

use mosaic_runtime::{Limits, Sandbox};
use tessera_ascii::{Grid, ImageRef, compose_codepoints, feature, render_structural};

const FACET_WASM: &[u8] = include_bytes!("facet_structural.wasm");
const CELL_ASPECT: f32 = 2.0;

fn grayscale(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> Vec<u8> {
    let mut buf = vec![0u8; w as usize * h as usize * 4];
    for y in 0..h {
        for x in 0..w {
            let v = f(x, y);
            let i = (y as usize * w as usize + x as usize) * 4;
            buf[i] = v;
            buf[i + 1] = v;
            buf[i + 2] = v;
            buf[i + 3] = 255;
        }
    }
    buf
}

fn assert_structural_matches_native(
    sandbox: &Sandbox,
    facet: &mosaic_runtime::Facet,
    w: u32,
    h: u32,
    data: &[u8],
    cols: u32,
) {
    let img = ImageRef::new(w, h, data).unwrap();
    let native = render_structural(&img, cols, CELL_ASPECT).unwrap();

    let grid = Grid::new(w, h, cols, CELL_ASPECT);
    let feats = feature::extract_structural(&img, &grid);
    let ncells = (feats.cols * feats.rows) as usize;
    let cps = sandbox
        .run_map(
            facet,
            Limits::default(),
            &feats.data,
            ncells,
            feats.stride as usize,
        )
        .unwrap();
    let sandboxed = compose_codepoints(feats.cols, feats.rows, &cps);

    assert_eq!(
        sandboxed, native,
        "structural sandboxed != native for {w}x{h}, cols={cols}"
    );
}

#[test]
fn structural_matches_native_vertical_edge() {
    let sandbox = Sandbox::new().unwrap();
    let facet = sandbox.compile(FACET_WASM).unwrap();
    let data = grayscale(240, 160, |x, _| if x < 120 { 0 } else { 255 });
    assert_structural_matches_native(&sandbox, &facet, 240, 160, &data, 60);
}

#[test]
fn structural_matches_native_horizontal_edge() {
    let sandbox = Sandbox::new().unwrap();
    let facet = sandbox.compile(FACET_WASM).unwrap();
    let data = grayscale(240, 160, |_, y| if y < 80 { 0 } else { 255 });
    assert_structural_matches_native(&sandbox, &facet, 240, 160, &data, 60);
}

/// Deterministic xorshift PRNG for a reproducible equivalence sweep.
struct Rng(u64);
impl Rng {
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 32) as u32
    }
    fn range(&mut self, lo: u32, hi: u32) -> u32 {
        lo + self.next_u32() % (hi - lo)
    }
}

/// Formal validation of the L2 claim: for many random images the untrusted
/// sandboxed structural Facet produces output byte-identical to the native engine.
#[test]
fn structural_matches_native_over_random_images() {
    let sandbox = Sandbox::new().unwrap();
    let facet = sandbox.compile(FACET_WASM).unwrap();
    let mut rng = Rng(0x51A7_C0DE_0BAD_F00D);
    for _ in 0..64 {
        let w = rng.range(4, 56);
        let h = rng.range(4, 44);
        let data: Vec<u8> = (0..(w * h * 4)).map(|_| rng.next_u32() as u8).collect();
        let cols = rng.range(8, 40);
        assert_structural_matches_native(&sandbox, &facet, w, h, &data, cols);
    }
}
