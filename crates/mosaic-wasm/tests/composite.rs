//! Native cross-engine composition (O4): real image-ASCII and audio-spectrogram token
//! grids, each produced by the sandboxed `facet-ramp`, composited via
//! `mosaic_core::composite`. Asserts the artifact is correctly shaped, deterministic, and
//! non-degenerate — the "two engines, one artifact" proof on the native/server path. The
//! browser equivalent is `crates/mosaic-wasm/test/composite.test.ts` against the golden.

use mosaic_core::composite::{Blend, Canvas, Layer};
use mosaic_runtime::{Facet, Limits, Sandbox};
use tessera_ascii::{Grid, ImageRef, feature as ascii_feature};
use tessera_spectral::{SignalRef, SpectroGrid, feature as spectral_feature};

const FACET_RAMP: &[u8] = include_bytes!("facet_ramp.wasm");
const SPACE: u32 = b' ' as u32;

/// Deterministic xorshift PRNG (broadband noise → a good spread of band energies).
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
    fn bipolar(&mut self) -> f32 {
        (self.next_u32() as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

fn image_tokens(sandbox: &Sandbox, facet: &Facet) -> (u32, u32, Vec<u32>) {
    let (w, h, cols) = (48u32, 24u32, 24u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let v = (((x + y) * 255) / (w + h - 2)) as u8;
            let i = ((y * w + x) * 4) as usize;
            rgba[i] = v;
            rgba[i + 1] = v;
            rgba[i + 2] = v;
            rgba[i + 3] = 255;
        }
    }
    let img = ImageRef::new(w, h, &rgba).unwrap();
    let grid = Grid::new(w, h, cols, 2.0);
    let feats = ascii_feature::extract(&img, &grid).unwrap();
    let ncells = (feats.cols * feats.rows) as usize;
    let tokens = sandbox
        .run_map(
            facet,
            Limits::default(),
            &feats.data,
            ncells,
            feats.stride as usize,
        )
        .unwrap();
    (feats.cols, feats.rows, tokens)
}

fn spectral_tokens(sandbox: &Sandbox, facet: &Facet) -> (u32, u32, Vec<u32>, Vec<f32>) {
    let mut rng = Rng(0x00C0_FFEE_5EED_0004);
    let samples: Vec<f32> = (0..4096).map(|_| rng.bipolar()).collect();
    let sig = SignalRef::new(&samples, 8000).unwrap();
    let grid = SpectroGrid::new(16, 256, 128, 120.0, 3600.0);
    let buf = spectral_feature::extract(&sig, &grid).unwrap();
    let ncells = (buf.cols * buf.rows) as usize;
    let tokens = sandbox
        .run_map(
            facet,
            Limits::default(),
            &buf.data,
            ncells,
            buf.stride as usize,
        )
        .unwrap();
    (buf.cols, buf.rows, tokens, buf.data)
}

#[test]
fn cross_engine_stack_is_shaped_deterministic_and_nondegenerate() {
    let sandbox = Sandbox::new().unwrap();
    let facet = sandbox.compile(FACET_RAMP).unwrap();
    let (ic, ir, itok) = image_tokens(&sandbox, &facet);
    let (sc, sr, stok, _energy) = spectral_tokens(&sandbox, &facet);

    let compose = || {
        let mut c = Canvas::new(ic.max(sc), ir + sr).unwrap();
        c.place(
            &Layer::keyed(ic, ir, itok.clone(), SPACE).unwrap(),
            0,
            0,
            Blend::Over,
        );
        c.place(
            &Layer::keyed(sc, sr, stok.clone(), SPACE).unwrap(),
            ir as i32,
            0,
            Blend::Over,
        );
        c.into_text(SPACE)
    };

    let text = compose();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len() as u32, ir + sr, "stacked height = both grids");
    for l in &lines {
        assert_eq!(l.chars().count() as u32, ic.max(sc), "canvas width");
    }
    let top_ink = lines[..ir as usize]
        .iter()
        .any(|l| l.chars().any(|ch| ch != ' '));
    let bottom_ink = lines[ir as usize..]
        .iter()
        .any(|l| l.chars().any(|ch| ch != ' '));
    assert!(
        top_ink && bottom_ink,
        "both engines must contribute ink to the single artifact"
    );
    assert_eq!(compose(), text, "composition is deterministic");
}

#[test]
fn stipple_by_energy_mixes_glyphs_and_background() {
    let sandbox = Sandbox::new().unwrap();
    let facet = sandbox.compile(FACET_RAMP).unwrap();
    let (sc, sr, stok, energy) = spectral_tokens(&sandbox, &facet);

    let compose = || {
        let mut c = Canvas::new(sc, sr).unwrap();
        c.place(
            &Layer::with_coverage(sc, sr, stok.clone(), energy.clone()).unwrap(),
            0,
            0,
            Blend::StippleOver,
        );
        c.into_text(b'.' as u32)
    };

    let text = compose();
    let glyphs: Vec<char> = text.chars().filter(|c| *c != '\n').collect();
    assert!(
        glyphs.contains(&'.'),
        "low-energy cells stipple to the background"
    );
    assert!(
        glyphs.iter().any(|&c| c != '.'),
        "high-energy cells show glyphs"
    );
    assert_eq!(compose(), text, "stipple is deterministic (ordered dither)");
}
