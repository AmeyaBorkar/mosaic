//! # mosaic-core
//!
//! The domain-agnostic core of **Mosaic** — the substrate layer from the project
//! vision. This crate defines the shapes every domain shares, so that a **Facet**
//! (a community-authored style) written against one **Tessera** (a domain engine)
//! is structurally comparable to a Facet in any other.
//!
//! The three layers (see `docs/architecture.md`):
//! - **Mosaic** — the platform substrate: registry, runtime, generated controls.
//! - **Tessera** — one engine per domain (ASCII, ANSI, halftone, data→art).
//! - **Facet** — the community style layer, many per Tessera.
//!
//! ## The engine contract (five slots)
//!
//! Every Tessera fills the same five slots. The modules below name them.
//!
//! | Slot | Question | Module |
//! |------|----------|--------|
//! | Input | What media does this engine take? | [`input`] |
//! | Unit | How is the media decomposed into pieces? | [`unit`] |
//! | Feature vocabulary | What may a unit measure about itself? | [`feature`] |
//! | Output primitive | What does one unit become? | [`output`] |
//! | Composition | How are the pieces reassembled? | [`compose`] |
//!
//! Two load-bearing decisions are now settled (see `docs/architecture.md`):
//! - **D5 / O1 — access model:** a unit is a pure function of features gathered
//!   over a bounded read-only neighborhood (radius R; R=0 = self-only), keeping
//!   the common path parallel and deterministic. Sequential feedback (e.g.
//!   error-diffusion dithering) is a separate, opt-in [`propagate`] capability.
//! - **D6 / O2 — first vocabulary (ASCII):** luminance + gradient + sub-cell
//!   structure. The concrete features live in the ASCII engine; here we define the
//!   [`feature`] *schema* shape that carries them across the Facet ABI.
//!
//! The [`manifest`] declares a Facet's user-facing parameters — the surface Mosaic
//! turns into controls automatically.

#![forbid(unsafe_code)]

/// Slot 1 — the media an engine accepts.
///
/// What kind of media a Tessera takes (an image, a dataset, …). The concrete
/// type is the Tessera's; kept abstract here pending a second domain (O5).
pub mod input {}

/// Slot 2 — how media is decomposed into workable units.
///
/// How a Tessera decomposes its input into units (a grid cell, a row, …), and
/// how units are addressed. The addressing space is domain-shaped (a 2-D grid
/// for ASCII, a 1-D sequence for data→art), so it stays abstract here (O5).
///
/// Access model (D5 / O1): a unit is evaluated as a pure function of its
/// [`crate::feature`]s, which may be gathered over a bounded neighborhood. The
/// Facet only ever sees the resulting features and returns one output, so
/// per-unit evaluation stays parallel and deterministic regardless of gather.
pub mod unit {}

/// Slot 3 — the feature vocabulary: what a unit may measure about itself.
///
/// A Tessera declares a [`FeatureSchema`]: the ordered, typed set of measurements
/// a unit exposes to a Facet. The runtime uses the schema to marshal each unit's
/// features into the flat `f32` buffer the (WASM) Facet reads — so the runtime is
/// generic while each Tessera defines its own concrete vocabulary.
///
/// For the first ASCII Tessera (D6 / O2) the fields are, illustratively:
/// `luminance` (L0, [`FeatureType::Scalar`]), `gradient` (L1,
/// [`FeatureType::Vector`] of magnitude+orientation, gathered), and `patch` (L2,
/// [`FeatureType::Patch`] sub-cell structure for glyph shape-matching).
pub mod feature {
    /// A Tessera's declared feature vocabulary: an ordered list of typed fields.
    ///
    /// Order is the ABI: fields lay out contiguously in the Facet's input buffer.
    /// Extend by appending, never by reordering, to keep existing Facets valid.
    #[derive(Debug, Clone, PartialEq)]
    pub struct FeatureSchema {
        pub fields: Vec<FeatureField>,
    }

    impl FeatureSchema {
        /// Total `f32` slots one unit's features occupy — the per-unit stride of
        /// the Facet input buffer.
        pub fn total_slots(&self) -> u32 {
            self.fields.iter().map(|f| f.ty.slots()).sum()
        }

        /// The widest neighbourhood any field reads (in units). The runtime pads
        /// the feature field by this radius so every unit can gather in-bounds.
        pub fn max_radius(&self) -> u16 {
            self.fields
                .iter()
                .map(|f| f.gather.radius())
                .max()
                .unwrap_or(0)
        }
    }

    /// One field in the vocabulary: an identifier, a wire type, and how far it
    /// reads around its unit.
    #[derive(Debug, Clone, PartialEq)]
    pub struct FeatureField {
        /// Identifier the Facet reads (e.g. `"luminance"`, `"gradient"`).
        pub key: String,
        pub ty: FeatureType,
        pub gather: Gather,
    }

    /// The wire type of a feature field — a small, deterministic numeric set that
    /// marshals cleanly and bit-reproducibly across the WASM boundary.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum FeatureType {
        /// A single `f32` (e.g. luminance, gradient magnitude).
        Scalar,
        /// A fixed-length vector of `f32` (e.g. mean RGB = 3, gradient = 2).
        Vector { len: u16 },
        /// A fixed 2-D patch of `f32` (e.g. an N×M sub-cell luminance patch, L2).
        Patch { rows: u16, cols: u16 },
    }

    impl FeatureType {
        /// Number of contiguous `f32` slots this field occupies.
        pub const fn slots(self) -> u32 {
            match self {
                FeatureType::Scalar => 1,
                FeatureType::Vector { len } => len as u32,
                FeatureType::Patch { rows, cols } => rows as u32 * cols as u32,
            }
        }
    }

    /// How far a feature reads around its unit (D5 / O1). `SelfOnly` is radius 0.
    /// Both variants are pure over the immutable input, so gather never breaks
    /// parallelism or determinism.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Gather {
        SelfOnly,
        /// Bounded neighbourhood; `radius` is measured in units.
        Neighborhood {
            radius: u16,
        },
    }

    impl Gather {
        /// Radius in units (0 for `SelfOnly`).
        pub const fn radius(self) -> u16 {
            match self {
                Gather::SelfOnly => 0,
                Gather::Neighborhood { radius } => radius,
            }
        }
    }
}

/// Slot 4 — the output primitive a single unit becomes.
///
/// What one unit maps to (a character, a colored glyph, a dot, a mark). Like
/// features, the concrete primitive is the Tessera's; the runtime marshals a
/// Facet's returned output per a declared output schema mirroring
/// [`crate::feature::FeatureSchema`].
pub mod output {}

/// Slot 5 — composition: how output primitives are reassembled into a whole.
///
/// How per-unit outputs recombine (a text grid, a raster, a plot). The text-grid
/// composition below is **domain-agnostic**: it turns any engine's per-unit `u32`
/// output tokens into a safe character grid. Every Tessera whose output primitive is
/// a Unicode glyph (ASCII, spectrogram, …) shares this one implementation, so the
/// untrusted-text boundary is defined once, in the substrate, and enforced
/// identically across domains rather than re-implemented per engine.
pub mod compose {
    /// Compose per-unit output codepoints (produced by a Facet) into a string,
    /// row-major with `\n` between rows.
    ///
    /// Untrusted Facet output is never assumed to be safe text. A codepoint is
    /// replaced with `U+FFFD` when it is not a valid Unicode scalar
    /// (`char::from_u32` fails) **or** when it is unsafe to emit — a C0/C1 control
    /// (including `ESC`, which would inject terminal escape sequences, and `LF`/`CR`,
    /// which would break the row/column grid) or a bidi/format override used for
    /// visual spoofing. Only printable glyphs cross the boundary; the row separators
    /// are the sole `\n` this function emits.
    ///
    /// Callers pass grid dimensions already capped by their engine's cell budget;
    /// this function never pre-allocates unboundedly from untrusted sizes.
    pub fn compose_codepoints(cols: u32, rows: u32, codepoints: &[u32]) -> String {
        let hint = (cols as usize)
            .saturating_mul(rows as usize)
            .saturating_add(rows as usize)
            .min(1 << 16);
        let mut out = String::with_capacity(hint);
        for row in 0..rows {
            for col in 0..cols {
                let idx = (row as usize * cols as usize) + col as usize;
                let ch = codepoints
                    .get(idx)
                    .and_then(|&c| char::from_u32(c))
                    .filter(|c| !is_unsafe_glyph(*c))
                    .unwrap_or('\u{FFFD}');
                out.push(ch);
            }
            if row + 1 < rows {
                out.push('\n');
            }
        }
        out
    }

    /// Whether a scalar value is unsafe to emit from untrusted Facet output and must
    /// be masked to `U+FFFD`: any C0/C1 control or `DEL` (`char::is_control`, covering
    /// `ESC`, `LF`, `CR`, …) and the bidirectional/format overrides used for visual
    /// spoofing. Rust's std cannot query the `Cf` general category without a Unicode
    /// table, so the well-known spoofing overrides are listed explicitly.
    fn is_unsafe_glyph(c: char) -> bool {
        c.is_control()
            || matches!(c,
                '\u{200E}' | '\u{200F}' | '\u{061C}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}')
    }

    #[cfg(test)]
    mod tests {
        use super::compose_codepoints;

        #[test]
        fn validates_untrusted_output() {
            // 'A', a surrogate (invalid), out-of-range (invalid), 'B' -> replacements.
            let cps = vec![0x41u32, 0xD800, 0x11_0000, 0x42];
            assert_eq!(compose_codepoints(2, 2, &cps), "A\u{FFFD}\n\u{FFFD}B");
            // Too few codepoints: missing cells become the replacement char, no panic.
            assert_eq!(compose_codepoints(2, 1, &[0x41]), "A\u{FFFD}");
        }

        #[test]
        fn masks_control_and_bidi() {
            // ESC, LF, DEL, and a RLO bidi override are all unsafe -> U+FFFD; 'A'
            // survives. This blocks terminal-escape injection and newline-driven grid
            // corruption from untrusted Facet output, once, for every engine.
            let cps = vec![0x1Bu32, 0x0A, 0x7F, 0x202E, 0x41];
            assert_eq!(
                compose_codepoints(5, 1, &cps),
                "\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}A"
            );
        }
    }
}

/// **Composition** across renders (O4) — combining whole renders into one artifact.
///
/// [`compose`] turns *one* engine's per-unit tokens into *one* text grid (slot 5). This
/// module is the layer above it: it composites *many* rendered grids — from the same
/// engine or from different ones — into a single artifact via a painter's-algorithm
/// [`Canvas`]. Because it operates only on already-produced `u32` output tokens it is
/// fully domain-agnostic (an image render and an audio render composite identically), and
/// it executes no untrusted code: [`Canvas::into_text`] routes every surviving cell
/// through [`compose::compose_codepoints`], so the untrusted-glyph boundary is preserved.
///
/// One primitive, [`Canvas::place`], unifies **overlay** (place at the origin), **layout
/// / tiling** (place at an offset), and **masking** (per-cell [`Layer`] coverage). A cell
/// holds one glyph, so there is no true alpha between glyphs — but partial coverage is
/// resolved by an ordered Bayer dither ([`Blend::StippleOver`]), so layers blend
/// perceptually without any impossible "half-glyph".
pub mod composite {
    use crate::compose::compose_codepoints;

    /// Upper bound on canvas cells, guarding against pathological allocations. Matches the
    /// engines' grid caps; a composed artifact never needs more.
    pub const MAX_CANVAS_CELLS: usize = 8_000_000;

    /// Errors from constructing composition inputs. Malformed sizes are values, not panics.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Error {
        /// A layer's `tokens`/`coverage` length did not equal `cols * rows`.
        LayerSizeMismatch {
            cols: u32,
            rows: u32,
            tokens: usize,
            coverage: usize,
        },
        /// Canvas dimensions were zero, overflowed, or exceeded [`MAX_CANVAS_CELLS`].
        CanvasTooLarge { cells: usize, max: usize },
    }

    impl core::fmt::Display for Error {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            match self {
                Error::LayerSizeMismatch {
                    cols,
                    rows,
                    tokens,
                    coverage,
                } => write!(
                    f,
                    "layer {cols}x{rows} needs {} tokens/coverage, got {tokens}/{coverage}",
                    (*cols as usize).saturating_mul(*rows as usize)
                ),
                Error::CanvasTooLarge { cells, max } => {
                    write!(
                        f,
                        "canvas has {cells} cells, outside the valid range (1..={max})"
                    )
                }
            }
        }
    }

    impl std::error::Error for Error {}

    /// How a [`Layer`] combines with what is already on the [`Canvas`] where they overlap.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Blend {
        /// Paint the layer where its coverage ≥ 0.5, over whatever is beneath.
        Over,
        /// Paint the layer where its coverage ≥ 0.5, but only where the canvas is still
        /// transparent — fill gaps beneath existing content.
        Under,
        /// Overwrite the canvas cell unconditionally with the layer's token and coverage,
        /// including clearing to transparent where the layer is transparent.
        Replace,
        /// Paint the layer using an ordered Bayer dither on its fractional coverage, so a
        /// partially-covered region stipples the layer over what is beneath.
        StippleOver,
    }

    fn cells(cols: u32, rows: u32) -> usize {
        (cols as usize).saturating_mul(rows as usize)
    }

    /// A produced render ready to composite: a `cols × rows` grid of output tokens with a
    /// per-cell coverage in `[0, 1]` (0 = transparent, 1 = opaque).
    #[derive(Debug, Clone)]
    pub struct Layer {
        cols: u32,
        rows: u32,
        tokens: Vec<u32>,
        coverage: Vec<f32>,
    }

    impl Layer {
        /// A fully opaque layer — every cell paints.
        pub fn opaque(cols: u32, rows: u32, tokens: Vec<u32>) -> Result<Layer, Error> {
            let n = cells(cols, rows);
            if tokens.len() != n {
                return Err(Error::LayerSizeMismatch {
                    cols,
                    rows,
                    tokens: tokens.len(),
                    coverage: n,
                });
            }
            let coverage = vec![1.0; n];
            Ok(Layer {
                cols,
                rows,
                tokens,
                coverage,
            })
        }

        /// A layer keyed on a `background` token: cells equal to it are transparent
        /// (coverage 0), all others opaque. The zero-ABI way to give an existing Facet's
        /// grid transparency — e.g. spaces become see-through.
        pub fn keyed(
            cols: u32,
            rows: u32,
            tokens: Vec<u32>,
            background: u32,
        ) -> Result<Layer, Error> {
            let n = cells(cols, rows);
            if tokens.len() != n {
                return Err(Error::LayerSizeMismatch {
                    cols,
                    rows,
                    tokens: tokens.len(),
                    coverage: n,
                });
            }
            let coverage = tokens
                .iter()
                .map(|&t| if t == background { 0.0 } else { 1.0 })
                .collect();
            Ok(Layer {
                cols,
                rows,
                tokens,
                coverage,
            })
        }

        /// A layer with explicit per-cell coverage — for an engine that emits a soft mask.
        pub fn with_coverage(
            cols: u32,
            rows: u32,
            tokens: Vec<u32>,
            coverage: Vec<f32>,
        ) -> Result<Layer, Error> {
            let n = cells(cols, rows);
            if tokens.len() != n || coverage.len() != n {
                return Err(Error::LayerSizeMismatch {
                    cols,
                    rows,
                    tokens: tokens.len(),
                    coverage: coverage.len(),
                });
            }
            Ok(Layer {
                cols,
                rows,
                tokens,
                coverage,
            })
        }

        pub fn cols(&self) -> u32 {
            self.cols
        }

        pub fn rows(&self) -> u32 {
            self.rows
        }
    }

    /// 4×4 ordered Bayer threshold matrix (values 0..16; normalized by 16 in use) for
    /// [`Blend::StippleOver`]. Position-based, so the stipple is deterministic and stable
    /// across the native and wasm builds.
    const BAYER4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

    /// A composition surface built up by [`Canvas::place`] calls, starting fully
    /// transparent. Painter's algorithm — later `place`s draw over earlier ones.
    pub struct Canvas {
        cols: u32,
        rows: u32,
        tokens: Vec<u32>,
        coverage: Vec<f32>,
    }

    impl Canvas {
        /// A new, fully transparent `cols × rows` canvas. Rejects a zero or oversized grid
        /// (> [`MAX_CANVAS_CELLS`]) as a value, never a panic.
        pub fn new(cols: u32, rows: u32) -> Result<Canvas, Error> {
            let n = cells(cols, rows);
            if n == 0 || n > MAX_CANVAS_CELLS {
                return Err(Error::CanvasTooLarge {
                    cells: n,
                    max: MAX_CANVAS_CELLS,
                });
            }
            Ok(Canvas {
                cols,
                rows,
                tokens: vec![0; n],
                coverage: vec![0.0; n],
            })
        }

        pub fn cols(&self) -> u32 {
            self.cols
        }

        pub fn rows(&self) -> u32 {
            self.rows
        }

        /// Composite `layer` onto the canvas with its top-left at (`row_off`, `col_off`),
        /// clipping to the canvas bounds. Offsets may be negative or push the layer partly
        /// off-canvas; only in-bounds cells are affected.
        pub fn place(&mut self, layer: &Layer, row_off: i32, col_off: i32, blend: Blend) {
            let canvas_cols = self.cols as i32;
            let canvas_rows = self.rows as i32;
            for lr in 0..layer.rows {
                let cr = lr as i32 + row_off;
                if cr < 0 || cr >= canvas_rows {
                    continue;
                }
                for lc in 0..layer.cols {
                    let cc = lc as i32 + col_off;
                    if cc < 0 || cc >= canvas_cols {
                        continue;
                    }
                    let li = (lr as usize) * (layer.cols as usize) + lc as usize;
                    let ci = (cr as usize) * (self.cols as usize) + cc as usize;
                    let cov = layer.coverage[li];
                    let tok = layer.tokens[li];
                    match blend {
                        Blend::Replace => {
                            self.tokens[ci] = tok;
                            self.coverage[ci] = cov;
                        }
                        Blend::Over => {
                            if cov >= 0.5 {
                                self.tokens[ci] = tok;
                                self.coverage[ci] = 1.0;
                            }
                        }
                        Blend::Under => {
                            if cov >= 0.5 && self.coverage[ci] < 0.5 {
                                self.tokens[ci] = tok;
                                self.coverage[ci] = 1.0;
                            }
                        }
                        Blend::StippleOver => {
                            let thr = BAYER4[(cr as usize) & 3][(cc as usize) & 3] as f32 / 16.0;
                            if cov > thr {
                                self.tokens[ci] = tok;
                                self.coverage[ci] = 1.0;
                            }
                        }
                    }
                }
            }
        }

        /// Resolve the canvas to output tokens: transparent cells (coverage < 0.5) become
        /// `background`.
        pub fn into_tokens(self, background: u32) -> Vec<u32> {
            self.tokens
                .iter()
                .zip(self.coverage.iter())
                .map(|(&t, &c)| if c >= 0.5 { t } else { background })
                .collect()
        }

        /// Resolve and compose to text: transparent cells become `background`, then the
        /// shared [`compose_codepoints`] validates every glyph (the untrusted-output
        /// boundary applies to a composed artifact exactly as to a single render).
        pub fn into_text(self, background: u32) -> String {
            let (cols, rows) = (self.cols, self.rows);
            let tokens = self.into_tokens(background);
            compose_codepoints(cols, rows, &tokens)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const SP: u32 = b' ' as u32;
        const A: u32 = b'A' as u32;
        const B: u32 = b'B' as u32;

        #[test]
        fn rejects_bad_sizes() {
            assert!(matches!(
                Layer::opaque(2, 2, vec![A; 3]),
                Err(Error::LayerSizeMismatch { .. })
            ));
            assert!(matches!(
                Canvas::new(0, 5),
                Err(Error::CanvasTooLarge { .. })
            ));
            assert!(matches!(
                Canvas::new(u32::MAX, u32::MAX),
                Err(Error::CanvasTooLarge { .. })
            ));
        }

        #[test]
        fn over_shows_through_transparent_cells() {
            // Bottom all 'B'; top is 'A' with a keyed-out (space) hole in the middle.
            let bottom = Layer::opaque(3, 1, vec![B, B, B]).unwrap();
            let top = Layer::keyed(3, 1, vec![A, SP, A], SP).unwrap();
            let mut c = Canvas::new(3, 1).unwrap();
            c.place(&bottom, 0, 0, Blend::Over);
            c.place(&top, 0, 0, Blend::Over);
            assert_eq!(c.into_text(SP), "ABA"); // the hole reveals the bottom 'B'
        }

        #[test]
        fn under_only_fills_gaps() {
            // Existing top with a hole; Under-place a full 'B' sheet — it shows only in the hole.
            let top = Layer::keyed(3, 1, vec![A, SP, A], SP).unwrap();
            let sheet = Layer::opaque(3, 1, vec![B, B, B]).unwrap();
            let mut c = Canvas::new(3, 1).unwrap();
            c.place(&top, 0, 0, Blend::Over);
            c.place(&sheet, 0, 0, Blend::Under);
            assert_eq!(c.into_text(SP), "ABA");
        }

        #[test]
        fn replace_overwrites_and_can_clear() {
            let a = Layer::opaque(2, 1, vec![A, A]).unwrap();
            // Replace with a keyed layer: 'B' stamps, the keyed-out cell clears to background.
            let b = Layer::keyed(2, 1, vec![B, SP], SP).unwrap();
            let mut c = Canvas::new(2, 1).unwrap();
            c.place(&a, 0, 0, Blend::Over);
            c.place(&b, 0, 0, Blend::Replace);
            assert_eq!(c.into_text(b'.' as u32), "B.");
        }

        #[test]
        fn place_clips_to_bounds() {
            let layer = Layer::opaque(2, 2, vec![A, A, A, A]).unwrap();
            let mut c = Canvas::new(3, 3).unwrap();
            // Straddle the bottom-right corner: only one cell lands in-bounds.
            c.place(&layer, 2, 2, Blend::Over);
            assert_eq!(c.into_text(SP), "   \n   \n  A");
            // A fully off-canvas placement is a no-op (negative and beyond).
            let mut c2 = Canvas::new(3, 3).unwrap();
            c2.place(&layer, -5, 0, Blend::Over);
            c2.place(&layer, 0, 9, Blend::Over);
            assert_eq!(c2.into_text(SP), "   \n   \n   ");
        }

        #[test]
        fn stipple_over_dithers_partial_coverage() {
            // Uniform 0.5 coverage → the 4×4 Bayer paints exactly the 8 cells with
            // threshold < 0.5 (values 0..7), a fixed checkerboard-like pattern.
            let tokens = vec![A; 16];
            let coverage = vec![0.5f32; 16];
            let layer = Layer::with_coverage(4, 4, tokens, coverage).unwrap();
            let mut c = Canvas::new(4, 4).unwrap();
            c.place(&layer, 0, 0, Blend::StippleOver);
            let text = c.into_text(SP);
            assert_eq!(text.chars().filter(|&ch| ch == 'A').count(), 8);
            // Deterministic and position-stable.
            let mut c2 = Canvas::new(4, 4).unwrap();
            c2.place(&layer, 0, 0, Blend::StippleOver);
            assert_eq!(c2.into_text(SP), text);
        }

        #[test]
        fn stipple_over_extremes() {
            let mut c = Canvas::new(4, 4).unwrap();
            // Coverage 1.0 everywhere → paints every cell (1.0 > max threshold 15/16).
            let full = Layer::with_coverage(4, 4, vec![A; 16], vec![1.0; 16]).unwrap();
            c.place(&full, 0, 0, Blend::StippleOver);
            assert!(c.into_text(SP).chars().all(|ch| ch == 'A' || ch == '\n'));
            // Coverage 0.0 everywhere → paints nothing (0.0 > 0 is false).
            let mut c2 = Canvas::new(4, 4).unwrap();
            let none = Layer::with_coverage(4, 4, vec![A; 16], vec![0.0; 16]).unwrap();
            c2.place(&none, 0, 0, Blend::StippleOver);
            assert!(c2.into_text(SP).chars().all(|ch| ch == ' ' || ch == '\n'));
        }

        #[test]
        fn into_text_masks_unsafe_tokens() {
            // A composed cell holding an ESC control codepoint is masked to U+FFFD, exactly
            // as a single render — composition never bypasses the untrusted-output boundary.
            let layer = Layer::opaque(2, 1, vec![0x1B, A]).unwrap();
            let mut c = Canvas::new(2, 1).unwrap();
            c.place(&layer, 0, 0, Blend::Over);
            assert_eq!(c.into_text(SP), "\u{FFFD}A");
        }

        #[test]
        fn layout_tiles_two_layers_at_offsets() {
            // Two 1×2 layers placed into a 2×2 canvas — layout, not overlay.
            let top = Layer::opaque(2, 1, vec![A, A]).unwrap();
            let bottom = Layer::opaque(2, 1, vec![B, B]).unwrap();
            let mut c = Canvas::new(2, 2).unwrap();
            c.place(&top, 0, 0, Blend::Over);
            c.place(&bottom, 1, 0, Blend::Over);
            assert_eq!(c.into_text(SP), "AA\nBB");
        }
    }
}

/// Opt-in sequential propagation (D5 / O1) — for the feedback class.
///
/// Most Facets are pure per-unit gather (parallel, deterministic). The one
/// genuinely-sequential pattern — feedback, e.g. error-diffusion dithering —
/// is confined here as an *opt-in* capability so it never taints the common
/// path.
///
/// A participating Facet does not scatter arbitrarily (that would break purity
/// and determinism). Instead it returns a *residual* alongside its output, and
/// the engine diffuses that residual to not-yet-processed units along a
/// declared kernel and traversal order. The Facet stays pure; the engine owns
/// the ordering, so the whole pass stays deterministic and reproducible.
///
/// The concrete capability type is designed alongside the first Facet that
/// needs it; the contract reserves the mechanism from day one.
pub mod propagate {}

/// A Facet's declared, user-facing parameters — the surface Mosaic renders into
/// controls automatically. The simple, config-level side of a style.
pub mod manifest {
    /// A Facet's full declared parameter surface. Mosaic generates one control per
    /// [`Param`] and passes the user's values to the Facet logic at render time.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Manifest {
        pub params: Vec<Param>,
    }

    /// One declared parameter: a stable key, a human label, optional help, and a
    /// typed control spec that drives the generated UI.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Param {
        /// Stable identifier the Facet logic reads (e.g. `"contrast"`).
        pub key: String,
        /// Human-facing label for the control (e.g. `"Contrast"`).
        pub label: String,
        /// Optional longer help/description text.
        pub help: Option<String>,
        /// The control's type, bounds, and default — the whole widget spec.
        pub control: Control,
    }

    /// The kind of control, its bounds, and its default value. Each variant maps
    /// to a specific auto-generated UI widget. Kept to broadly-universal control
    /// types; domain-specific controls (e.g. a glyph ramp) are added by Tesserae.
    #[derive(Debug, Clone, PartialEq)]
    pub enum Control {
        /// Continuous slider.
        Float {
            default: f64,
            min: f64,
            max: f64,
            step: Option<f64>,
        },
        /// Integer stepper / slider.
        Int { default: i64, min: i64, max: i64 },
        /// Checkbox.
        Bool { default: bool },
        /// Single choice from a fixed set (dropdown / segmented control).
        Choice {
            default: String,
            options: Vec<ChoiceOption>,
        },
        /// Free text, length-bounded.
        Text { default: String, max_len: u32 },
        /// Color picker.
        Color { default: Rgba },
    }

    /// One option in a [`Control::Choice`].
    #[derive(Debug, Clone, PartialEq)]
    pub struct ChoiceOption {
        pub value: String,
        pub label: String,
    }

    /// An 8-bit-per-channel RGBA color.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Rgba {
        pub r: u8,
        pub g: u8,
        pub b: u8,
        pub a: u8,
    }
}

#[cfg(test)]
mod tests {
    use crate::feature::{FeatureField, FeatureSchema, FeatureType, Gather};

    /// The buffer stride is the sum of every field's slots — this is the layout
    /// the Facet ABI relies on. Exercised with the ASCII vocabulary shape
    /// (D6 / O2): luminance (L0), gradient (L1, gathered), sub-cell patch (L2).
    #[test]
    fn feature_buffer_stride_sums_field_slots() {
        let schema = FeatureSchema {
            fields: vec![
                FeatureField {
                    key: "luminance".into(),
                    ty: FeatureType::Scalar,
                    gather: Gather::SelfOnly,
                },
                FeatureField {
                    key: "gradient".into(),
                    ty: FeatureType::Vector { len: 2 },
                    gather: Gather::Neighborhood { radius: 1 },
                },
                FeatureField {
                    key: "patch".into(),
                    ty: FeatureType::Patch { rows: 4, cols: 4 },
                    gather: Gather::SelfOnly,
                },
            ],
        };
        assert_eq!(schema.total_slots(), 1 + 2 + 16);
        assert_eq!(schema.max_radius(), 1);
    }

    #[test]
    fn schema_invariants_hold_over_random_fields() {
        // Deterministic xorshift PRNG (reproducible sweep, no dependency).
        let mut state: u64 = 0xDEAD_BEEF_CAFE_F00D;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 40) as u16
        };
        for _ in 0..500 {
            let n = (next() % 8) as usize;
            let mut fields = Vec::new();
            let mut expect_slots = 0u32;
            let mut expect_radius = 0u16;
            for i in 0..n {
                let ty = match next() % 3 {
                    0 => FeatureType::Scalar,
                    1 => FeatureType::Vector {
                        len: 1 + next() % 8,
                    },
                    _ => FeatureType::Patch {
                        rows: 1 + next() % 8,
                        cols: 1 + next() % 8,
                    },
                };
                let radius = next() % 4;
                let gather = if radius == 0 {
                    Gather::SelfOnly
                } else {
                    Gather::Neighborhood { radius }
                };
                expect_slots += ty.slots();
                expect_radius = expect_radius.max(radius);
                fields.push(FeatureField {
                    key: format!("f{i}"),
                    ty,
                    gather,
                });
            }
            let schema = FeatureSchema { fields };
            // total_slots is exactly the sum of field slots; max_radius the max gather.
            assert_eq!(schema.total_slots(), expect_slots);
            assert_eq!(schema.max_radius(), expect_radius);
        }
    }
}
