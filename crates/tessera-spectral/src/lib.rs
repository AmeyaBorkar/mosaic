//! # tessera-spectral
//!
//! Mosaic's **second** Tessera: audio (PCM) → spectrogram text art.
//!
//! Its whole reason to exist is to test the one claim the platform is built on —
//! that the five-slot engine contract ([`mosaic_core`]) is *universal*, not secretly
//! shaped around images. This engine fills the same five slots with a completely
//! different Input (a 1-D signal, not an RGBA image) and a new feature vocabulary
//! (per-band spectral energy, not luminance), yet reuses — unchanged — the
//! substrate's Facet ABI, sandbox, and text composition:
//!
//! ```text
//! PCM samples → STFT grid (time × frequency) → per-cell band energy → map → text
//! ```
//!
//! The payoff is concrete and tested (`tests/sandboxed_spectral.rs`): the *existing
//! image Facets* — `facet-ramp` (a gather Facet) and `facet-dither` (a propagation
//! Facet), the very WASM binaries authored for the ASCII engine — run **unmodified**
//! over this engine's spectral features and produce output byte-identical to the
//! native references here. A Facet is a domain-agnostic feature-vector → token
//! function; with two engines that is now a passing test rather than an assertion.
//!
//! ## Method
//!
//! Columns are STFT frames (time), rows are frequency bands (log-spaced between
//! `fmin` and `fmax`, the perceptually natural axis for audio; band 0 is the lowest
//! center frequency and is laid out at the *bottom*, so the image reads like a
//! conventional spectrogram). Each frame is Hann-windowed and each band's energy is
//! measured by a [Goertzel](https://en.wikipedia.org/wiki/Goertzel_algorithm) filter,
//! then the grid is normalized to `[0, 1]` by its peak. Band energy is a single
//! `Scalar` feature per cell, so a scalar-consuming Facet reads it exactly as it reads
//! ASCII luminance.
//!
//! ## Determinism
//!
//! The extractor uses the same discipline as [`tessera_ascii`]: every transcendental
//! goes through `libm` (`cosf` for the window and band coefficients, `powf` for the
//! band centers) and no `mul_add` is used, so the STFT is bit-reproducible and ready
//! for the native ↔ wasm parity path. (Browser bindings + a native/wasm golden for
//! *this* extractor are a follow-on, mirroring the ASCII engine; the cross-domain
//! Facet proof below needs only the native engine plus the real sandbox.)
//!
//! Pure, `#![forbid(unsafe_code)]`, and no panics on malformed input (overflow-checked
//! sizing, budget-gated allocation).

#![forbid(unsafe_code)]

pub use error::Error;
pub use grid::SpectroGrid;
pub use signal::SignalRef;

// The text-grid composer is the substrate's shared slot-5 primitive. Re-export it (as
// `tessera-ascii` does) so this engine's consumers — and the sandboxed-Facet tests —
// compose untrusted output through the one boundary, and so the render fns below can
// call it directly.
pub use mosaic_core::compose::compose_codepoints;

/// Upper bound on grid cells (frames × bands), guarding against pathological
/// allocations from an absurd band count or a very long signal. Matches
/// `tessera-ascii`'s cap; a spectrogram never needs more.
pub const MAX_CELLS: usize = 8_000_000;

/// Upper bound on the `f32` feature buffer for one render, in **bytes** — sized so the
/// buffer plus a Facet's output fits inside `mosaic-runtime`'s 16 MiB per-execution
/// memory cap. Byte-aware, matching the ASCII engine's budget.
pub const MAX_FEATURE_BYTES: usize = 8 * 1024 * 1024;

/// Render a signal to spectrogram text using the density-ramp mapping.
///
/// This is the **native reference** for running the image `facet-ramp` over spectral
/// features: [`render::density_glyph`] mirrors that Facet's ramp exactly, so the
/// sandboxed Facet and this function agree byte-for-byte (see the sandboxed tests).
pub fn render_spectral(signal: &SignalRef, grid: &SpectroGrid) -> Result<String, Error> {
    let buf = feature::extract(signal, grid)?;
    let codepoints: Vec<u32> = buf.data.iter().map(|&v| render::density_glyph(v)).collect();
    Ok(compose_codepoints(buf.cols, buf.rows, &codepoints))
}

/// Render a signal to spectrogram text using **error-diffusion dithering** — the
/// propagation method class (D5). Each cell's band energy is quantized to 1 bit and
/// the error is diffused via the shared [`dither::floyd_steinberg`], the same routine
/// the ASCII engine and the `dither` Facet run. Native reference for running the image
/// `facet-dither` over spectral features.
pub fn render_spectral_dither(signal: &SignalRef, grid: &SpectroGrid) -> Result<String, Error> {
    let mut buf = feature::extract(signal, grid)?;
    let ncells = buf.data.len();
    let mut out = vec![0u32; ncells];
    dither::floyd_steinberg(
        &mut buf.data,
        buf.cols as usize,
        buf.rows as usize,
        buf.stride as usize,
        &mut out,
    );
    Ok(compose_codepoints(buf.cols, buf.rows, &out))
}

/// Errors returned by the engine. Malformed input is always a value, never a panic.
pub mod error {
    /// Everything that can go wrong turning a signal into a spectrogram.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Error {
        /// The sample buffer was empty.
        EmptySignal,
        /// The sample rate was zero.
        ZeroSampleRate,
        /// The band range was invalid: not `0 < fmin < fmax <= sample_rate / 2`.
        InvalidFrequencyRange,
        /// Grid dimensions overflowed when computing a buffer size.
        DimensionOverflow,
        /// The grid exceeded [`crate::MAX_CELLS`].
        TooManyCells { cells: usize, max: usize },
        /// The feature buffer would exceed [`crate::MAX_FEATURE_BYTES`].
        FeatureBufferTooLarge { bytes: usize, max: usize },
    }

    impl core::fmt::Display for Error {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            match self {
                Error::EmptySignal => write!(f, "the signal must contain at least one sample"),
                Error::ZeroSampleRate => write!(f, "sample rate must be greater than zero"),
                Error::InvalidFrequencyRange => write!(
                    f,
                    "band range must satisfy 0 < fmin < fmax <= sample_rate/2 (Nyquist)"
                ),
                Error::DimensionOverflow => {
                    write!(f, "dimensions overflow when computing a buffer size")
                }
                Error::TooManyCells { cells, max } => {
                    write!(f, "grid has {cells} cells, exceeding the maximum of {max}")
                }
                Error::FeatureBufferTooLarge { bytes, max } => write!(
                    f,
                    "feature buffer is {bytes} bytes, exceeding the maximum of {max}"
                ),
            }
        }
    }

    impl std::error::Error for Error {}
}

/// Slot 1 (Input) — a borrowed, validated block of mono PCM samples.
pub mod signal {
    use super::error::Error;

    /// A borrowed block of mono PCM samples with its sample rate.
    ///
    /// Sample amplitude is conventionally in `[-1, 1]` but is not required to be; the
    /// extractor windows and measures energy either way. Construct with
    /// [`SignalRef::new`], which rejects an empty buffer or a zero sample rate.
    #[derive(Debug, Clone, Copy)]
    pub struct SignalRef<'a> {
        samples: &'a [f32],
        sample_rate: u32,
    }

    impl<'a> SignalRef<'a> {
        /// Validate and wrap a sample buffer.
        pub fn new(samples: &'a [f32], sample_rate: u32) -> Result<Self, Error> {
            if samples.is_empty() {
                return Err(Error::EmptySignal);
            }
            if sample_rate == 0 {
                return Err(Error::ZeroSampleRate);
            }
            Ok(Self {
                samples,
                sample_rate,
            })
        }

        pub fn sample_rate(&self) -> u32 {
            self.sample_rate
        }

        pub fn len(&self) -> usize {
            self.samples.len()
        }

        /// Always `false` after construction (an empty buffer is rejected); provided
        /// for API completeness alongside [`SignalRef::len`].
        pub fn is_empty(&self) -> bool {
            self.samples.is_empty()
        }

        /// Sample at index `n`, or `0.0` past the end — zero-padding the final frame so
        /// every frame reads a full window without bounds errors.
        pub fn sample(&self, n: usize) -> f32 {
            self.samples.get(n).copied().unwrap_or(0.0)
        }
    }
}

/// Slot 2 (Unit) — the STFT partitioning: frames (time) × bands (frequency).
pub mod grid {
    /// How a signal is partitioned into a time × frequency grid.
    ///
    /// Holds only the partitioning scheme (band count, window, hop, band range); the
    /// number of time frames and the concrete band frequencies depend on the signal
    /// (its length and sample rate) and are derived at extraction. All size fields are
    /// clamped to at least 1.
    #[derive(Debug, Clone, Copy)]
    pub struct SpectroGrid {
        bands: u32,
        win: u32,
        hop: u32,
        fmin: f32,
        fmax: f32,
    }

    impl SpectroGrid {
        /// Build a grid of `bands` log-spaced frequency bands measured over a `win`-
        /// sample window advanced by `hop` samples per frame, covering `[fmin, fmax]`
        /// Hz. `bands`, `win`, and `hop` are clamped to at least 1; the band range is
        /// validated (against the signal's Nyquist) at extraction.
        pub fn new(bands: u32, win: u32, hop: u32, fmin: f32, fmax: f32) -> SpectroGrid {
            SpectroGrid {
                bands: bands.max(1),
                win: win.max(1),
                hop: hop.max(1),
                fmin,
                fmax,
            }
        }

        pub fn bands(&self) -> u32 {
            self.bands
        }

        pub fn win(&self) -> u32 {
            self.win
        }

        pub fn hop(&self) -> u32 {
            self.hop
        }

        pub fn fmin(&self) -> f32 {
            self.fmin
        }

        pub fn fmax(&self) -> f32 {
            self.fmax
        }

        /// Number of time frames (grid columns) for a signal of `nsamples`: at least 1
        /// (a short signal yields one zero-padded frame), otherwise one per hop that
        /// still starts within the signal. Saturating, so a huge signal cannot
        /// overflow — the cell budget rejects anything oversized downstream.
        pub fn frames(&self, nsamples: usize) -> u32 {
            let extra = nsamples.saturating_sub(self.win as usize);
            let frames = 1 + extra / self.hop as usize;
            frames.min(u32::MAX as usize) as u32
        }

        /// Center frequency (Hz) of `band` (0-based, low → high), geometrically spaced
        /// between `fmin` and `fmax`. Single source of truth for the band axis, used by
        /// the extractor and available to callers.
        pub fn band_center_hz(&self, band: u32) -> f32 {
            if self.bands <= 1 {
                self.fmin
            } else {
                let t = band as f32 / (self.bands - 1) as f32;
                self.fmin * libm::powf(self.fmax / self.fmin, t)
            }
        }
    }
}

/// Slot 3 (Feature vocabulary) — per-cell spectral band energy and its extraction.
pub mod feature {
    use super::error::Error;
    use super::grid::SpectroGrid;
    use super::signal::SignalRef;
    use mosaic_core::feature::{FeatureField, FeatureSchema, FeatureType, Gather};

    /// Per-cell features, row-major over the `frames × bands` grid, each cell one
    /// `f32`: normalized band energy in `[0, 1]`. `stride` is 1 — the same flat scalar
    /// layout a gather Facet expects, which is exactly why image scalar Facets read it
    /// unchanged.
    #[derive(Debug, Clone)]
    pub struct FeatureBuffer {
        /// Time frames.
        pub cols: u32,
        /// Frequency bands.
        pub rows: u32,
        /// `f32` slots per cell (always 1 for this vocabulary).
        pub stride: u32,
        pub data: Vec<f32>,
    }

    /// The declared feature vocabulary: a single self-only scalar, `band_energy`. A
    /// different domain and a different measurement, carried by the same
    /// [`mosaic_core::feature::FeatureSchema`] shape as the ASCII vocabulary.
    pub fn vocabulary() -> FeatureSchema {
        FeatureSchema {
            fields: vec![FeatureField {
                key: "band_energy".into(),
                ty: FeatureType::Scalar,
                gather: Gather::SelfOnly,
            }],
        }
    }

    /// Reject a feature buffer that overflows, exceeds [`crate::MAX_CELLS`] cells, or
    /// exceeds [`crate::MAX_FEATURE_BYTES`], *before* it is allocated — so a
    /// pathological grid can never drive a huge or aborting allocation.
    fn check_feature_budget(ncells: usize, stride: u32) -> Result<(), Error> {
        if ncells > crate::MAX_CELLS {
            return Err(Error::TooManyCells {
                cells: ncells,
                max: crate::MAX_CELLS,
            });
        }
        let bytes = ncells
            .checked_mul(stride as usize)
            .and_then(|slots| slots.checked_mul(4))
            .ok_or(Error::DimensionOverflow)?;
        if bytes > crate::MAX_FEATURE_BYTES {
            return Err(Error::FeatureBufferTooLarge {
                bytes,
                max: crate::MAX_FEATURE_BYTES,
            });
        }
        Ok(())
    }

    /// A Hann window of length `win` (`0.5 - 0.5·cos(2πn/(win-1))`), via `libm::cosf`
    /// for cross-target determinism. A degenerate 1-sample window is all-ones.
    fn hann(win: usize) -> Vec<f32> {
        if win <= 1 {
            return vec![1.0; win.max(1)];
        }
        let denom = (win - 1) as f32;
        (0..win)
            .map(|n| 0.5 - 0.5 * libm::cosf(core::f32::consts::TAU * n as f32 / denom))
            .collect()
    }

    /// Measure normalized band energy over every `(frame, band)` cell.
    ///
    /// For each band, a Goertzel filter at the band's center frequency accumulates the
    /// Hann-windowed frame; its magnitude is the raw energy. The grid is then
    /// normalized by its global peak into `[0, 1]`. Bands are laid into rows with the
    /// lowest frequency at the bottom (`row = bands-1-band`), so the output reads like
    /// a conventional spectrogram.
    pub fn extract(signal: &SignalRef, grid: &SpectroGrid) -> Result<FeatureBuffer, Error> {
        let sample_rate = signal.sample_rate() as f32;
        let nyquist = sample_rate / 2.0;
        if !(grid.fmin() > 0.0 && grid.fmin() < grid.fmax() && grid.fmax() <= nyquist) {
            return Err(Error::InvalidFrequencyRange);
        }

        let bands = grid.bands();
        let frames = grid.frames(signal.len());
        let ncells = (frames as usize)
            .checked_mul(bands as usize)
            .ok_or(Error::DimensionOverflow)?;
        check_feature_budget(ncells, 1)?;

        let win = grid.win() as usize;
        let hop = grid.hop() as usize;
        let window = hann(win);

        // Pass 1 — raw Goertzel magnitude per (band, frame), tracking the peak.
        let mut data = vec![0.0f32; ncells];
        let mut peak = 0.0f32;
        for band in 0..bands {
            let fc = grid.band_center_hz(band);
            // Goertzel coefficient for normalized frequency fc/sample_rate.
            let coeff = 2.0 * libm::cosf(core::f32::consts::TAU * fc / sample_rate);
            let row = (bands - 1 - band) as usize; // lowest frequency at the bottom
            let row_base = row * frames as usize;
            for f in 0..frames as usize {
                let start = f * hop;
                let mut s1 = 0.0f32;
                let mut s2 = 0.0f32;
                for (n, &w) in window.iter().enumerate() {
                    let x = signal.sample(start + n) * w;
                    let s = x + coeff * s1 - s2;
                    s2 = s1;
                    s1 = s;
                }
                // Goertzel power → magnitude; clamp tiny negative FP noise before sqrt.
                let power = s1 * s1 + s2 * s2 - coeff * s1 * s2;
                let mag = power.max(0.0).sqrt();
                data[row_base + f] = mag;
                if mag > peak {
                    peak = mag;
                }
            }
        }

        // Pass 2 — normalize to [0, 1] by the peak (silence stays all-zero).
        if peak > 0.0 {
            for v in &mut data {
                *v = (*v / peak).clamp(0.0, 1.0);
            }
        }

        Ok(FeatureBuffer {
            cols: frames,
            rows: bands,
            stride: 1,
            data,
        })
    }
}

/// Slots 4 & 5 (Output primitive + Composition) — the ramp mapping and parameters.
pub mod render {
    use mosaic_core::manifest::{Control, Manifest, Param};

    /// The glyph ramp, sparse → dense — byte-identical to `facet-ramp`'s ramp, so the
    /// native reference render matches the sandboxed image Facet run over these
    /// features.
    pub const RAMP: &[u8] = b" .:-=+*#%@";

    /// Map a normalized band energy in `[0, 1]` to a ramp glyph codepoint.
    ///
    /// This mirrors `facet-ramp`'s `density_glyph` **exactly** — same ramp, same
    /// `(l·(n-1) + 0.5) as usize` truncation — which is what lets the untrusted image
    /// Facet and this native reference agree byte-for-byte on the same features.
    pub fn density_glyph(v: f32) -> u32 {
        let l = v.clamp(0.0, 1.0);
        let idx = (l * (RAMP.len() as f32 - 1.0) + 0.5) as usize;
        RAMP[idx.min(RAMP.len() - 1)] as u32
    }

    /// The parameter surface Mosaic would render into controls — proving the manifest
    /// model is domain-agnostic by declaring a second engine's parameters through it.
    pub fn manifest() -> Manifest {
        Manifest {
            params: vec![
                Param {
                    key: "bands".into(),
                    label: "Frequency bands".into(),
                    help: Some("Number of log-spaced frequency rows.".into()),
                    control: Control::Int {
                        default: 48,
                        min: 1,
                        max: 512,
                    },
                },
                Param {
                    key: "fmin".into(),
                    label: "Min frequency (Hz)".into(),
                    help: Some("Lowest band center frequency.".into()),
                    control: Control::Float {
                        default: 55.0,
                        min: 1.0,
                        max: 20_000.0,
                        step: Some(1.0),
                    },
                },
                Param {
                    key: "fmax".into(),
                    label: "Max frequency (Hz)".into(),
                    help: Some("Highest band center frequency (≤ Nyquist).".into()),
                    control: Control::Float {
                        default: 8_000.0,
                        min: 1.0,
                        max: 20_000.0,
                        step: Some(1.0),
                    },
                },
                Param {
                    key: "win".into(),
                    label: "Window size".into(),
                    help: Some("STFT window length in samples.".into()),
                    control: Control::Int {
                        default: 1024,
                        min: 16,
                        max: 16_384,
                    },
                },
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pure tone of `n` samples at `freq` Hz, generated with `libm::sinf`.
    fn tone(sample_rate: u32, freq: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| libm::sinf(core::f32::consts::TAU * freq * i as f32 / sample_rate as f32))
            .collect()
    }

    fn default_grid() -> SpectroGrid {
        SpectroGrid::new(24, 1024, 512, 80.0, 3600.0)
    }

    #[test]
    fn rejects_empty_and_zero_rate() {
        assert_eq!(SignalRef::new(&[], 8000).unwrap_err(), Error::EmptySignal);
        assert_eq!(
            SignalRef::new(&[0.0, 1.0], 0).unwrap_err(),
            Error::ZeroSampleRate
        );
    }

    #[test]
    fn rejects_invalid_frequency_range() {
        let sig = SignalRef::new(&[0.1; 2048], 8000).unwrap();
        // fmax above Nyquist (4000).
        let g = SpectroGrid::new(16, 1024, 512, 100.0, 5000.0);
        assert_eq!(
            feature::extract(&sig, &g).unwrap_err(),
            Error::InvalidFrequencyRange
        );
        // fmin >= fmax.
        let g = SpectroGrid::new(16, 1024, 512, 2000.0, 2000.0);
        assert_eq!(
            feature::extract(&sig, &g).unwrap_err(),
            Error::InvalidFrequencyRange
        );
    }

    #[test]
    fn silence_renders_all_spaces() {
        let sig = SignalRef::new(&[0.0f32; 4096], 8000).unwrap();
        let grid = default_grid();
        let out = render_spectral(&sig, &grid).unwrap();
        assert!(out.chars().filter(|c| *c != '\n').all(|c| c == ' '));
        // The dither reference agrees: no energy anywhere means all dark cells.
        let out_d = render_spectral_dither(&sig, &grid).unwrap();
        assert!(out_d.chars().filter(|c| *c != '\n').all(|c| c == ' '));
    }

    #[test]
    fn tone_lights_the_expected_band() {
        let grid = default_grid();
        let sample_rate = 8000;
        // A tone exactly at band 12's center should peak in that band.
        let b_star = 12u32;
        let freq = grid.band_center_hz(b_star);
        let sig_samples = tone(sample_rate, freq, 4096);
        let sig = SignalRef::new(&sig_samples, sample_rate).unwrap();

        let buf = feature::extract(&sig, &grid).unwrap();
        // Argmax cell.
        let (max_idx, &max_val) = buf
            .data
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        let max_row = max_idx / buf.cols as usize;
        let expected_row = (grid.bands() - 1 - b_star) as usize;
        assert!(
            (max_row as i64 - expected_row as i64).abs() <= 1,
            "tone peaked at row {max_row}, expected ~{expected_row}"
        );
        assert!((max_val - 1.0).abs() < 1e-6, "peak should normalize to 1.0");

        // The peak cell normalizes to 1.0, so the render contains the densest glyph.
        let out = render_spectral(&sig, &grid).unwrap();
        assert!(
            out.contains('@'),
            "a tone should light at least one full cell"
        );
    }

    #[test]
    fn output_shape_matches_grid() {
        let sig_samples = tone(8000, 440.0, 5000);
        let sig = SignalRef::new(&sig_samples, 8000).unwrap();
        let grid = default_grid();
        let buf = feature::extract(&sig, &grid).unwrap();
        assert_eq!(buf.rows, grid.bands());
        assert_eq!(buf.cols, grid.frames(sig.len()));

        let out = render_spectral(&sig, &grid).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len() as u32, buf.rows);
        for line in &lines {
            assert_eq!(line.chars().count() as u32, buf.cols);
        }
    }

    #[test]
    fn determinism() {
        let sig_samples = tone(8000, 523.25, 6000);
        let sig = SignalRef::new(&sig_samples, 8000).unwrap();
        let grid = default_grid();
        assert_eq!(
            feature::extract(&sig, &grid).unwrap().data,
            feature::extract(&sig, &grid).unwrap().data
        );
        assert_eq!(
            render_spectral(&sig, &grid).unwrap(),
            render_spectral(&sig, &grid).unwrap()
        );
        assert_eq!(
            render_spectral_dither(&sig, &grid).unwrap(),
            render_spectral_dither(&sig, &grid).unwrap()
        );
    }

    #[test]
    fn handles_short_signal_without_panic() {
        // Fewer samples than the window: exactly one zero-padded frame, no panic.
        let sig = SignalRef::new(&[0.5, -0.5, 0.25], 8000).unwrap();
        let grid = default_grid();
        let buf = feature::extract(&sig, &grid).unwrap();
        assert_eq!(buf.cols, 1);
        assert_eq!(buf.rows, grid.bands());
        assert!(render_spectral(&sig, &grid).is_ok());
    }

    #[test]
    fn dither_stipples_and_is_deterministic() {
        // Broadband noise fills many bands with mid-level energy; 1-bit error diffusion
        // must stipple into a mix of both glyphs (impossible for pure per-cell gather).
        let mut state: u64 = 0xA5A5_1234_DEAD_0001;
        let noise: Vec<f32> = (0..8192)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                ((state >> 40) as f32 / 0xFF_FFFF as f32) - 0.5
            })
            .collect();
        let sig = SignalRef::new(&noise, 8000).unwrap();
        let grid = default_grid();
        let out = render_spectral_dither(&sig, &grid).unwrap();
        let glyphs: Vec<char> = out.chars().filter(|c| *c != '\n').collect();
        assert!(glyphs.contains(&'@'));
        assert!(glyphs.contains(&' '));
        assert_eq!(render_spectral_dither(&sig, &grid).unwrap(), out);
    }

    #[test]
    fn extract_rejects_oversized_grid() {
        // A tiny signal but an absurd band count: the feature-byte budget must reject
        // it with a clean Error before allocating (the crate's no-panic contract),
        // and before running any Goertzel pass.
        let sig = SignalRef::new(&[0.0f32; 8], 8000).unwrap();
        let grid = SpectroGrid::new(2_100_000, 1024, 512, 80.0, 3600.0);
        assert!(matches!(
            feature::extract(&sig, &grid),
            Err(Error::FeatureBufferTooLarge { .. })
        ));
    }

    #[test]
    fn vocabulary_matches_core_schema() {
        let schema = feature::vocabulary();
        assert_eq!(schema.total_slots(), 1); // one scalar per cell
        assert_eq!(schema.max_radius(), 0); // self-only
    }

    #[test]
    fn manifest_is_well_formed() {
        let m = render::manifest();
        assert_eq!(m.params.len(), 4);
        assert!(m.params.iter().any(|p| p.key == "bands"));
    }
}
