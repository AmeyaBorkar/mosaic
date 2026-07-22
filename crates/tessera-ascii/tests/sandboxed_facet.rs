//! End-to-end proof: the real `facet-ramp` WASM module, run inside
//! `mosaic-runtime`'s sandbox, produces byte-identical ASCII to the native engine
//! path — a faithful, untrusted, sandboxed port of the density + edge method.
//!
//! The fixture `facet_ramp.wasm` is generated from `facets/ramp`; rebuild with:
//!   cargo build --manifest-path facets/ramp/Cargo.toml --target wasm32-unknown-unknown --release
//! and copy it next to this file.

use mosaic_runtime::{Limits, Sandbox};
use tessera_ascii::{Grid, ImageRef, Options, compose_codepoints, feature, render_ascii};

const FACET_WASM: &[u8] = include_bytes!("facet_ramp.wasm");

/// Left half black, right half white — a vertical edge (gradient dir 0 → `|`).
fn vertical_split(w: u32, h: u32) -> Vec<u8> {
    grayscale(w, h, |x, _| if x < w / 2 { 0 } else { 255 })
}

/// Top half black, bottom white — a horizontal edge (gradient dir π/2 → `-`).
fn horizontal_split(w: u32, h: u32) -> Vec<u8> {
    grayscale(w, h, |_, y| if y < h / 2 { 0 } else { 255 })
}

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

/// Render `data` both natively and through the sandboxed Facet, asserting they
/// match. These images use only axis-aligned edges and solid regions, so both
/// paths agree exactly (no float-reduction ambiguity at diagonal bins).
fn assert_sandboxed_matches_native(w: u32, h: u32, data: &[u8]) {
    let img = ImageRef::new(w, h, data).unwrap();
    let opts = Options {
        cols: 60,
        ..Options::default()
    };

    let native = render_ascii(&img, &opts).unwrap();

    let grid = Grid::new(w, h, opts.cols, opts.cell_aspect);
    let feats = feature::extract(&img, &grid).unwrap();
    let ncells = (feats.cols * feats.rows) as usize;

    let sandbox = Sandbox::new().unwrap();
    let facet = sandbox.compile(FACET_WASM).unwrap();
    let codepoints = sandbox
        .run_map(
            &facet,
            Limits::default(),
            &feats.data,
            ncells,
            feats.stride as usize,
        )
        .unwrap();
    let sandboxed = compose_codepoints(feats.cols, feats.rows, &codepoints);

    assert_eq!(sandboxed, native);
}

#[test]
fn sandboxed_facet_matches_native_vertical_edge() {
    assert_sandboxed_matches_native(240, 160, &vertical_split(240, 160));
}

#[test]
fn sandboxed_facet_matches_native_horizontal_edge() {
    assert_sandboxed_matches_native(240, 160, &horizontal_split(240, 160));
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

/// Formal validation of the core claim: for many random images, the untrusted
/// sandboxed Facet produces output byte-identical to the native engine — including
/// noisy diagonal edges, where the two paths reduce the gradient angle differently
/// (`rem_euclid` vs. a `while`-loop) yet must agree.
#[test]
fn sandboxed_matches_native_over_random_images() {
    let sandbox = Sandbox::new().unwrap();
    let facet = sandbox.compile(FACET_WASM).unwrap();
    let mut rng = Rng(0x0DDB_1A5E_5BAD_5EED);
    for _ in 0..64 {
        let w = rng.range(4, 64);
        let h = rng.range(4, 48);
        let data: Vec<u8> = (0..(w * h * 4)).map(|_| rng.next_u32() as u8).collect();
        let img = ImageRef::new(w, h, &data).unwrap();
        let opts = Options {
            cols: rng.range(8, 48),
            ..Options::default()
        };

        let native = render_ascii(&img, &opts).unwrap();

        let grid = Grid::new(w, h, opts.cols, opts.cell_aspect);
        let feats = feature::extract(&img, &grid).unwrap();
        let ncells = (feats.cols * feats.rows) as usize;
        let codepoints = sandbox
            .run_map(
                &facet,
                Limits::default(),
                &feats.data,
                ncells,
                feats.stride as usize,
            )
            .unwrap();
        let sandboxed = compose_codepoints(feats.cols, feats.rows, &codepoints);

        assert_eq!(
            sandboxed, native,
            "sandboxed != native for {w}x{h}, cols={}",
            opts.cols
        );
    }
}
