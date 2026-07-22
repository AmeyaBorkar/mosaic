//! End-to-end proof for the **propagation** method (D5): the real `facet-dither` WASM
//! module, run via `run_map_2d`, produces byte-identical ASCII to native
//! `render_dither`. Both call the shared `dither::floyd_steinberg`, so the untrusted
//! preview equals the render — even though the method is sequential (error feedback).
//!
//! The fixture `facet_dither.wasm` is generated from `facets/dither`; rebuild:
//!   RUSTFLAGS="-C link-arg=--max-memory=16777216" \
//!     cargo build --manifest-path facets/dither/Cargo.toml --target wasm32-unknown-unknown --release
//! and copy it next to this file.

use mosaic_runtime::{Limits, Sandbox};
use tessera_ascii::{Grid, ImageRef, compose_codepoints, feature, render_dither};

const FACET_WASM: &[u8] = include_bytes!("facet_dither.wasm");
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

fn assert_dither_matches_native(
    sandbox: &Sandbox,
    facet: &mosaic_runtime::Facet,
    w: u32,
    h: u32,
    data: &[u8],
    cols: u32,
) {
    let img = ImageRef::new(w, h, data).unwrap();
    let native = render_dither(&img, cols, CELL_ASPECT).unwrap();

    let grid = Grid::new(w, h, cols, CELL_ASPECT);
    let feats = feature::extract(&img, &grid).unwrap();
    let cps = sandbox
        .run_map_2d(
            facet,
            Limits::default(),
            &feats.data,
            feats.cols as usize,
            feats.rows as usize,
            feats.stride as usize,
        )
        .unwrap();
    let sandboxed = compose_codepoints(feats.cols, feats.rows, &cps);

    assert_eq!(
        sandboxed, native,
        "dither sandboxed != native for {w}x{h}, cols={cols}"
    );
}

#[test]
fn dither_matches_native_vertical_edge() {
    let sandbox = Sandbox::new().unwrap();
    let facet = sandbox.compile(FACET_WASM).unwrap();
    let data = grayscale(240, 160, |x, _| if x < 120 { 0 } else { 255 });
    assert_dither_matches_native(&sandbox, &facet, 240, 160, &data, 60);
}

#[test]
fn dither_matches_native_horizontal_edge() {
    let sandbox = Sandbox::new().unwrap();
    let facet = sandbox.compile(FACET_WASM).unwrap();
    let data = grayscale(240, 160, |_, y| if y < 80 { 0 } else { 255 });
    assert_dither_matches_native(&sandbox, &facet, 240, 160, &data, 60);
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

/// Formal validation of the propagation claim: for many random images the untrusted
/// sandboxed dither Facet (`run_map_2d`) produces output byte-identical to native
/// `render_dither` — the sequential error-diffusion accumulation is bit-identical
/// across the native and wasm targets.
#[test]
fn dither_matches_native_over_random_images() {
    let sandbox = Sandbox::new().unwrap();
    let facet = sandbox.compile(FACET_WASM).unwrap();
    let mut rng = Rng(0xD177_3E00_5EED_1234);
    for _ in 0..64 {
        let w = rng.range(4, 56);
        let h = rng.range(4, 44);
        let data: Vec<u8> = (0..(w * h * 4)).map(|_| rng.next_u32() as u8).collect();
        let cols = rng.range(8, 40);
        assert_dither_matches_native(&sandbox, &facet, w, h, &data, cols);
    }
}
