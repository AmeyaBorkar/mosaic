//! **Contract-universality proof (O5).**
//!
//! The `facet-ramp` (gather) and `facet-dither` (propagation) WASM modules were
//! authored for the *image* engine. Here the exact same binaries — byte-identical to
//! `crates/tessera-ascii/tests/` (same SHA-256) — run **unmodified** inside
//! `mosaic-runtime`'s sandbox over `tessera-spectral`'s **audio** features, and produce
//! output byte-identical to this engine's native references (`render_spectral`,
//! `render_spectral_dither`).
//!
//! That equality is the claim the whole platform rests on made concrete: a Facet is a
//! domain-agnostic `feature-vector → token` function. It reads slot 0 of each cell and
//! neither knows nor cares that the scalar came from image luminance in one engine and
//! from spectral band energy in another. Two engines now share one Facet, one sandbox,
//! and one composition boundary.
//!
//! Fixtures rebuild from `facets/ramp` and `facets/dither` (the ASCII engine's build
//! commands), copied next to this file.

use mosaic_runtime::{Facet, Limits, Sandbox};
use tessera_spectral::{
    SignalRef, SpectroGrid, compose_codepoints, feature, render_spectral, render_spectral_dither,
};

const RAMP_WASM: &[u8] = include_bytes!("facet_ramp.wasm");
const DITHER_WASM: &[u8] = include_bytes!("facet_dither.wasm");

/// Deterministic xorshift PRNG — reproducible signals without `Math.random`.
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
    /// A sample in `[-1, 1)`.
    fn bipolar(&mut self) -> f32 {
        (self.next_u32() as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// A pure tone of `n` samples at `freq` Hz. `std::f32::sin` is fine here: the native
/// render and the sandboxed Facet consume the *same* extracted feature buffer, so the
/// only requirement is that the two paths agree — not cross-target sample generation.
fn tone(sample_rate: u32, freq: f32, n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| (core::f32::consts::TAU * freq * i as f32 / sample_rate as f32).sin())
        .collect()
}

/// Run `facet-ramp` (gather ABI) over the signal's spectral features and assert the
/// sandboxed glyphs equal the native `render_spectral`.
fn assert_ramp_matches_native(
    sandbox: &Sandbox,
    facet: &Facet,
    samples: &[f32],
    sample_rate: u32,
    grid: &SpectroGrid,
) {
    let sig = SignalRef::new(samples, sample_rate).unwrap();
    let native = render_spectral(&sig, grid).unwrap();

    let buf = feature::extract(&sig, grid).unwrap();
    let ncells = (buf.cols * buf.rows) as usize;
    let cps = sandbox
        .run_map(
            facet,
            Limits::default(),
            &buf.data,
            ncells,
            buf.stride as usize,
        )
        .unwrap();
    let sandboxed = compose_codepoints(buf.cols, buf.rows, &cps);

    assert_eq!(
        sandboxed, native,
        "image ramp Facet over spectral features != native ({}x{} grid)",
        buf.cols, buf.rows
    );
}

/// Run `facet-dither` (propagation ABI, `run2d`) over the signal's spectral features
/// and assert the sandboxed glyphs equal the native `render_spectral_dither`. The
/// feature buffer fed to the sandbox is a fresh extraction (un-dithered); the guest
/// diffuses its own copy, exactly as the native reference does.
fn assert_dither_matches_native(
    sandbox: &Sandbox,
    facet: &Facet,
    samples: &[f32],
    sample_rate: u32,
    grid: &SpectroGrid,
) {
    let sig = SignalRef::new(samples, sample_rate).unwrap();
    let native = render_spectral_dither(&sig, grid).unwrap();

    let buf = feature::extract(&sig, grid).unwrap();
    let cps = sandbox
        .run_map_2d(
            facet,
            Limits::default(),
            &buf.data,
            buf.cols as usize,
            buf.rows as usize,
            buf.stride as usize,
        )
        .unwrap();
    let sandboxed = compose_codepoints(buf.cols, buf.rows, &cps);

    assert_eq!(
        sandboxed, native,
        "image dither Facet over spectral features != native ({}x{} grid)",
        buf.cols, buf.rows
    );
}

#[test]
fn image_ramp_facet_renders_a_tone() {
    let sandbox = Sandbox::new().unwrap();
    let ramp = sandbox.compile(RAMP_WASM).unwrap();
    let grid = SpectroGrid::new(32, 1024, 256, 60.0, 3800.0);
    // A tone at band 16's center — a real spectrogram, not a blank grid.
    let samples = tone(8000, grid.band_center_hz(16), 6000);

    // Guard against a vacuous "all blank matches all blank" pass.
    let native = render_spectral(&SignalRef::new(&samples, 8000).unwrap(), &grid).unwrap();
    let distinct: std::collections::BTreeSet<char> =
        native.chars().filter(|c| *c != '\n').collect();
    assert!(
        distinct.len() >= 2,
        "expected a non-degenerate spectrogram, got glyphs {distinct:?}"
    );

    assert_ramp_matches_native(&sandbox, &ramp, &samples, 8000, &grid);
}

#[test]
fn image_dither_facet_renders_noise() {
    let sandbox = Sandbox::new().unwrap();
    let dither = sandbox.compile(DITHER_WASM).unwrap();
    let grid = SpectroGrid::new(24, 512, 256, 80.0, 3600.0);
    let mut rng = Rng(0xD177_3E00_5EED_9999);
    let samples: Vec<f32> = (0..8192).map(|_| rng.bipolar()).collect();
    assert_dither_matches_native(&sandbox, &dither, &samples, 8000, &grid);
}

/// Formal validation: for many random signals across sample rates and grid shapes,
/// both untrusted image Facets — gather and propagation — produce output
/// byte-identical to the native spectral references. One binary, two domains.
#[test]
fn image_facets_match_native_over_many_signals() {
    let sandbox = Sandbox::new().unwrap();
    let ramp = sandbox.compile(RAMP_WASM).unwrap();
    let dither = sandbox.compile(DITHER_WASM).unwrap();
    let mut rng = Rng(0x5EED_A11D_C0DE_1234);

    let rates = [8000u32, 16000, 22050, 44100];
    for _ in 0..32 {
        let sample_rate = rates[(rng.next_u32() % 4) as usize];
        let nyquist = sample_rate as f32 / 2.0;

        let n = rng.range(256, 8192) as usize;
        let samples: Vec<f32> = (0..n).map(|_| rng.bipolar()).collect();

        let bands = rng.range(4, 64);
        let win = rng.range(64, 2048);
        let hop = rng.range(32, win);
        let fmin = rng.range(30, 200) as f32;
        // Keep 0 < fmin < fmax <= Nyquist.
        let fmax = (rng.range(1000, 6000) as f32)
            .min(nyquist - 1.0)
            .max(fmin + 50.0);
        let grid = SpectroGrid::new(bands, win, hop, fmin, fmax);

        assert_ramp_matches_native(&sandbox, &ramp, &samples, sample_rate, &grid);
        assert_dither_matches_native(&sandbox, &dither, &samples, sample_rate, &grid);
    }
}
