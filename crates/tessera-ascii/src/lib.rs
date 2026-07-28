//! # tessera-ascii
//!
//! Mosaic's first **Tessera**: images → ASCII text.
//!
//! Vertical slice proving the pipeline end-to-end and validating the
//! [`mosaic_core`] contract against a real domain (the first engine; contract
//! *universality* across domains is O5, proven by `tessera-spectral`):
//!
//! ```text
//! RGBA buffer → grid of cells → features (L0 luminance, L1 gradient, L2 structure) → map → text
//! ```
//!
//! - **L0** — per-cell mean luminance → a density ramp.
//! - **L1** — a Sobel gradient over the cell luminance grid (each cell reading its
//!   8 neighbors: a **radius-1 gather**, the first concrete exercise of decision
//!   D5/O1) → directional glyphs (`- / | \`) on strong edges.
//! - **L2** — sub-cell structural matching ([`render_structural`] / `feature::extract_structural`),
//!   each cell reduced to a luminance patch and matched to the closest glyph via the shared
//!   [`glyph_atlas`]. The propagation `render_dither` path (D5) is implemented here too.
//!
//! The vocabulary is declared as a [`mosaic_core::feature::FeatureSchema`] and
//! features are laid out in that schema's buffer, so L2 was added additively. The Facet's parameters are a
//! [`mosaic_core::manifest::Manifest`] ([`render::manifest`]), exercising the
//! auto-generated-controls surface.
//!
//! The engine is pure and deterministic, forbids `unsafe`, and validates all
//! inputs (overflow-checked sizing, no panics on malformed data).

#![forbid(unsafe_code)]

pub use error::Error;
pub use grid::Grid;
pub use image::ImageRef;
pub use render::{DEFAULT_RAMP, Options};

// The untrusted-output → text-grid composition is a domain-agnostic substrate
// primitive (Mosaic slot 5), so it lives in `mosaic-core` and is shared with every
// other engine. Re-exported here so `tessera_ascii::compose_codepoints` — used by the
// wasm bridge, the sandboxed-Facet tests, and the examples — keeps its path.
pub use mosaic_core::compose::compose_codepoints;

/// Upper bound on grid cells, guarding against pathological allocations from
/// absurd column counts. Generous: 8M cells is far beyond any real ASCII render.
pub const MAX_CELLS: usize = 8_000_000;

/// Upper bound on the `f32` feature buffer produced for one render, in **bytes**.
/// Unlike [`MAX_CELLS`] (a cell *count*), this is byte-aware, so it holds across both
/// the stride-8 (L0+L1+position+colour) and stride-64 (L2) vocabularies — an 8M-cell L2 grid would
/// otherwise allocate ~2 GB. Sized so the buffer plus a Facet's output fits inside
/// `mosaic-runtime`'s 16 MiB per-execution memory cap.
pub const MAX_FEATURE_BYTES: usize = 8 * 1024 * 1024;

/// Render an image to ASCII text using the density + edge Facet.
///
/// Returns an [`Error`] on invalid options rather than panicking. The input
/// [`ImageRef`] is already validated at construction.
pub fn render_ascii(image: &ImageRef, opts: &Options) -> Result<String, Error> {
    if opts.cols == 0 {
        return Err(Error::ZeroColumns);
    }
    if opts.ramp.is_empty() {
        return Err(Error::EmptyRamp);
    }
    let grid = Grid::new(image.width(), image.height(), opts.cols, opts.cell_aspect);
    let cells = (grid.cols() as usize)
        .checked_mul(grid.rows() as usize)
        .ok_or(Error::DimensionOverflow)?;
    if cells > MAX_CELLS {
        return Err(Error::TooManyCells {
            cells,
            max: MAX_CELLS,
        });
    }
    let buf = feature::extract(image, &grid)?;
    render::compose(&buf, opts)
}

/// Render an image to ASCII using the **L2 structural** method (D6): each cell is
/// reduced to a sub-cell luminance patch and matched to the closest glyph in the
/// shared [`glyph_atlas`]. Density and structure both fall out of that nearest-glyph
/// rule. This is the native reference for the `structural` Facet — it composes the
/// exact per-cell codepoints the Facet produces, using the same shared matcher.
pub fn render_structural(image: &ImageRef, cols: u32, cell_aspect: f32) -> Result<String, Error> {
    if cols == 0 {
        return Err(Error::ZeroColumns);
    }
    let grid = Grid::new(image.width(), image.height(), cols, cell_aspect);
    let cells = (grid.cols() as usize)
        .checked_mul(grid.rows() as usize)
        .ok_or(Error::DimensionOverflow)?;
    if cells > MAX_CELLS {
        return Err(Error::TooManyCells {
            cells,
            max: MAX_CELLS,
        });
    }
    let buf = feature::extract_structural(image, &grid)?;
    let stride = buf.stride as usize;
    let mut codepoints = Vec::with_capacity(cells);
    for i in 0..cells {
        let start = i * stride;
        codepoints.push(glyph_atlas::match_glyph(&buf.data[start..start + stride]));
    }
    Ok(compose_codepoints(buf.cols, buf.rows, &codepoints))
}

/// Render an image to ASCII using **error-diffusion dithering** — the propagation
/// method class (D5) the parallel gather model cannot express. Each cell's luminance
/// is quantized to 1 bit and the quantization error is diffused to later cells via
/// the shared [`dither::floyd_steinberg`], so a region of flat grey stipples into a
/// mix of glyphs. This is the native reference for the `dither` Facet — the same
/// shared routine both run, so preview == render.
pub fn render_dither(image: &ImageRef, cols: u32, cell_aspect: f32) -> Result<String, Error> {
    if cols == 0 {
        return Err(Error::ZeroColumns);
    }
    let grid = Grid::new(image.width(), image.height(), cols, cell_aspect);
    let cells = (grid.cols() as usize)
        .checked_mul(grid.rows() as usize)
        .ok_or(Error::DimensionOverflow)?;
    if cells > MAX_CELLS {
        return Err(Error::TooManyCells {
            cells,
            max: MAX_CELLS,
        });
    }
    let mut buf = feature::extract(image, &grid)?;
    let mut out = vec![0u32; cells];
    dither::floyd_steinberg(
        &mut buf.data,
        buf.cols as usize,
        buf.rows as usize,
        buf.stride as usize,
        &mut out,
    );
    Ok(compose_codepoints(buf.cols, buf.rows, &out))
}

/// Render an image to **braille** sub-cell art — no Facet. Each terminal cell becomes a 2×4
/// grid of braille dots (`U+2800`–`U+28FF`), so the effective resolution is `2·cols × 4·rows`,
/// roughly 8× the density render. A dot is raised where its sub-cell's mean luminance is bright
/// (`≥ 0.5`) — the same bright→dense convention as the density ramp. Deterministic integer
/// thresholding over exact `f32` means, so the browser render (`renderBraille`) is bit-identical
/// (preview == render). Braille glyphs are printable and pass `compose_codepoints` unmasked.
pub fn render_braille(image: &ImageRef, cols: u32, cell_aspect: f32) -> Result<String, Error> {
    if cols == 0 {
        return Err(Error::ZeroColumns);
    }
    let grid = Grid::new(image.width(), image.height(), cols, cell_aspect);
    let cells = (grid.cols() as usize)
        .checked_mul(grid.rows() as usize)
        .ok_or(Error::DimensionOverflow)?;
    if cells > MAX_CELLS {
        return Err(Error::TooManyCells {
            cells,
            max: MAX_CELLS,
        });
    }
    // Unicode braille dot bit per sub-cell `[row][col]` of the 2×4 grid: the left column holds
    // dots 1,2,3,7 (bits 0,1,2,6), the right column dots 4,5,6,8 (bits 3,4,5,7).
    const DOT_BITS: [[u32; 2]; 4] = [[0, 3], [1, 4], [2, 5], [6, 7]];
    let mut codepoints = Vec::with_capacity(cells);
    for row in 0..grid.rows() {
        for col in 0..grid.cols() {
            let (x0, x1, y0, y1) = grid.cell_bounds(col, row);
            let mut bits = 0u32;
            for sr in 0..4u32 {
                let (sy0, sy1) = sub_span(y0, y1, sr, 4);
                for sc in 0..2u32 {
                    let (sx0, sx1) = sub_span(x0, x1, sc, 2);
                    if sub_cell_is_lit(image, sx0, sx1, sy0, sy1) {
                        bits |= 1 << DOT_BITS[sr as usize][sc as usize];
                    }
                }
            }
            codepoints.push(0x2800 + bits);
        }
    }
    Ok(compose_codepoints(grid.cols(), grid.rows(), &codepoints))
}

/// The `idx`-th of `div` equal integer sub-spans of `[lo, hi)`, as `[a, b)`. Uses a `u64`
/// intermediate so the width × index product cannot overflow `u32` on a huge image.
fn sub_span(lo: u32, hi: u32, idx: u32, div: u32) -> (u32, u32) {
    let len = u64::from(hi - lo);
    let a = lo + (len * u64::from(idx) / u64::from(div)) as u32;
    let b = lo + (len * u64::from(idx + 1) / u64::from(div)) as u32;
    (a, b)
}

/// Whether a braille sub-cell reads as "lit": its mean luminance is `≥ 0.5`. An empty span
/// (a sub-cell narrower than a pixel) samples the nearest pixel clamped inside the image, so a
/// grid finer than the image never panics — it just repeats edge samples.
fn sub_cell_is_lit(image: &ImageRef, x0: u32, x1: u32, y0: u32, y1: u32) -> bool {
    let mut sum = 0.0f32;
    let mut count = 0u32;
    for y in y0..y1 {
        for x in x0..x1 {
            sum += image.luma(x, y);
            count += 1;
        }
    }
    let mean = if count > 0 {
        sum / count as f32
    } else {
        let px = x0.min(image.width().saturating_sub(1));
        let py = y0.min(image.height().saturating_sub(1));
        image.luma(px, py)
    };
    mean >= 0.5
}

/// Errors returned by the engine. Malformed input is always a value, never a panic.
pub mod error {
    /// Everything that can go wrong rendering an image to ASCII.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Error {
        /// Image width or height was zero.
        EmptyImage,
        /// The RGBA buffer length did not equal `width * height * 4`.
        BufferSizeMismatch { expected: usize, actual: usize },
        /// Image or grid dimensions overflowed when computing a buffer size.
        DimensionOverflow,
        /// Requested output columns was zero.
        ZeroColumns,
        /// The glyph ramp was empty.
        EmptyRamp,
        /// The grid exceeded [`crate::MAX_CELLS`].
        TooManyCells { cells: usize, max: usize },
        /// The feature buffer would exceed [`crate::MAX_FEATURE_BYTES`].
        FeatureBufferTooLarge { bytes: usize, max: usize },
    }

    impl core::fmt::Display for Error {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            match self {
                Error::EmptyImage => {
                    write!(f, "image width and height must both be non-zero")
                }
                Error::BufferSizeMismatch { expected, actual } => write!(
                    f,
                    "RGBA buffer size mismatch: expected {expected} bytes, got {actual}"
                ),
                Error::DimensionOverflow => {
                    write!(f, "dimensions overflow when computing a buffer size")
                }
                Error::ZeroColumns => write!(f, "output columns must be greater than zero"),
                Error::EmptyRamp => {
                    write!(f, "the glyph ramp must contain at least one character")
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

/// Slot 1 (Input) — a borrowed, validated RGBA image.
pub mod image {
    use super::error::Error;

    /// A borrowed, row-major, 8-bit RGBA image (4 bytes per pixel).
    ///
    /// Construct with [`ImageRef::new`], which validates the buffer length so all
    /// later pixel access is in-bounds by construction.
    #[derive(Debug, Clone, Copy)]
    pub struct ImageRef<'a> {
        width: u32,
        height: u32,
        rgba: &'a [u8],
    }

    impl<'a> ImageRef<'a> {
        /// Validate and wrap an RGBA buffer. `rgba.len()` must equal
        /// `width * height * 4`.
        pub fn new(width: u32, height: u32, rgba: &'a [u8]) -> Result<Self, Error> {
            if width == 0 || height == 0 {
                return Err(Error::EmptyImage);
            }
            let expected = (width as usize)
                .checked_mul(height as usize)
                .and_then(|px| px.checked_mul(4))
                .ok_or(Error::DimensionOverflow)?;
            if rgba.len() != expected {
                return Err(Error::BufferSizeMismatch {
                    expected,
                    actual: rgba.len(),
                });
            }
            Ok(Self {
                width,
                height,
                rgba,
            })
        }

        pub fn width(&self) -> u32 {
            self.width
        }

        pub fn height(&self) -> u32 {
            self.height
        }

        /// Rec. 709 luma of the pixel at `(x, y)`, in `[0, 1]`, computed in the
        /// sRGB-gamma domain (the convention text art expects). Alpha is ignored.
        ///
        /// Callers must keep `x < width` and `y < height`; the grid guarantees this.
        pub fn luma(&self, x: u32, y: u32) -> f32 {
            let idx = ((y as usize * self.width as usize) + x as usize) * 4;
            let r = self.rgba[idx] as f32 / 255.0;
            let g = self.rgba[idx + 1] as f32 / 255.0;
            let b = self.rgba[idx + 2] as f32 / 255.0;
            0.2126 * r + 0.7152 * g + 0.0722 * b
        }

        /// The raw 8-bit RGBA of the pixel at `(x, y)` — the source colour, for a coloured
        /// render (see [`crate::color`]). Callers must keep `x < width`, `y < height`.
        pub fn rgba(&self, x: u32, y: u32) -> [u8; 4] {
            let idx = ((y as usize * self.width as usize) + x as usize) * 4;
            [
                self.rgba[idx],
                self.rgba[idx + 1],
                self.rgba[idx + 2],
                self.rgba[idx + 3],
            ]
        }
    }
}

/// Colour output: pair the source image's colour with the glyph grid.
///
/// Colour is computed by the engine from the image, never by the (untrusted) Facet — it is a
/// deterministic integer mean of the source pixels, so a coloured render is bit-identical
/// native vs wasm (`preview == render`). Two modes:
///
/// - **half-block** ([`render_halfblock`]) — each cell is `▀` (U+2580) with foreground = the
///   mean colour of its top pixel-half and background = its bottom half, doubling vertical
///   resolution: true coloured "pixel art", no Facet involved.
/// - **glyph colour** ([`extract_cell_colors`]) — one mean colour per cell, to tint the
///   glyphs a Facet chose (colourise ASCII art).
///
/// Colours are packed RGBA in a `u32`, little-endian: `r | g<<8 | b<<16 | a<<24` (byte 0 is
/// red), the layout a canvas `ImageData` uses.
pub mod color {
    use super::error::Error;
    use super::grid::Grid;
    use super::image::ImageRef;

    /// The half-block glyph (`▀`, U+2580) every [`render_halfblock`] cell uses.
    pub const HALF_BLOCK: u32 = 0x2580;

    /// A coloured half-block render: `cols × rows` cells, each drawn as [`HALF_BLOCK`] with
    /// `fg[i]` over `bg[i]` (packed RGBA). Effective pixel resolution is `cols × 2·rows`.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct HalfBlock {
        pub cols: u32,
        pub rows: u32,
        /// Top-half mean colour per cell (the glyph foreground).
        pub fg: Vec<u32>,
        /// Bottom-half mean colour per cell (the glyph background).
        pub bg: Vec<u32>,
    }

    fn pack_rgba(r: u32, g: u32, b: u32, a: u32) -> u32 {
        r | (g << 8) | (b << 16) | (a << 24)
    }

    /// Deterministic integer mean colour of the pixels in `[x0, x1) × [y0, y1)`, packed RGBA.
    /// An empty range yields transparent black (`0`). `pub(crate)` so the feature extractor can
    /// surface the same mean colour as the `r`/`g`/`b` vocabulary slots a Facet reads — one
    /// source of truth for "the cell's colour", shared with the tint / half-block render.
    pub(crate) fn mean_color(image: &ImageRef, x0: u32, x1: u32, y0: u32, y1: u32) -> u32 {
        let (mut r, mut g, mut b, mut a, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
        for y in y0..y1 {
            for x in x0..x1 {
                let [pr, pg, pb, pa] = image.rgba(x, y);
                r += u64::from(pr);
                g += u64::from(pg);
                b += u64::from(pb);
                a += u64::from(pa);
                n += 1;
            }
        }
        if n == 0 {
            return 0;
        }
        pack_rgba(
            (r / n) as u32,
            (g / n) as u32,
            (b / n) as u32,
            (a / n) as u32,
        )
    }

    fn cell_count(grid: &Grid) -> Result<usize, Error> {
        let ncells = (grid.cols() as usize)
            .checked_mul(grid.rows() as usize)
            .ok_or(Error::DimensionOverflow)?;
        if ncells > crate::MAX_CELLS {
            return Err(Error::TooManyCells {
                cells: ncells,
                max: crate::MAX_CELLS,
            });
        }
        Ok(ncells)
    }

    /// Render `image` as coloured half-blocks over `grid`: each cell is `▀` with its top and
    /// bottom pixel-halves' mean colours. This is the coloured-pixel-art path — no Facet.
    pub fn render_halfblock(image: &ImageRef, grid: &Grid) -> Result<HalfBlock, Error> {
        cell_count(grid)?;
        let (cols, rows) = (grid.cols(), grid.rows());
        let mut fg = Vec::with_capacity(cols as usize * rows as usize);
        let mut bg = Vec::with_capacity(cols as usize * rows as usize);
        for row in 0..rows {
            for col in 0..cols {
                let (x0, x1, y0, y1) = grid.cell_bounds(col, row);
                if y1 - y0 <= 1 {
                    // A one-pixel-tall cell: both halves are that pixel row.
                    let c = mean_color(image, x0, x1, y0, y1);
                    fg.push(c);
                    bg.push(c);
                } else {
                    let ymid = y0 + (y1 - y0) / 2;
                    fg.push(mean_color(image, x0, x1, y0, ymid));
                    bg.push(mean_color(image, x0, x1, ymid, y1));
                }
            }
        }
        Ok(HalfBlock { cols, rows, fg, bg })
    }

    /// One mean colour per cell (`cols × rows`, row-major, packed RGBA) — to tint the glyphs a
    /// Facet produced (colourise its ASCII art).
    pub fn extract_cell_colors(image: &ImageRef, grid: &Grid) -> Result<Vec<u32>, Error> {
        cell_count(grid)?;
        let mut out = Vec::with_capacity(grid.cols() as usize * grid.rows() as usize);
        for row in 0..grid.rows() {
            for col in 0..grid.cols() {
                let (x0, x1, y0, y1) = grid.cell_bounds(col, row);
                out.push(mean_color(image, x0, x1, y0, y1));
            }
        }
        Ok(out)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn halfblock_splits_top_and_bottom() {
            // 2×2: top row red, bottom row blue. One cell (cell_aspect 1.0 -> rows 1).
            let rgba = [
                255, 0, 0, 255, 255, 0, 0, 255, // y=0
                0, 0, 255, 255, 0, 0, 255, 255, // y=1
            ];
            let image = ImageRef::new(2, 2, &rgba).unwrap();
            let grid = Grid::new(2, 2, 1, 1.0);
            let hb = render_halfblock(&image, &grid).unwrap();
            assert_eq!((hb.cols, hb.rows), (1, 1));
            assert_eq!(hb.fg, vec![pack_rgba(255, 0, 0, 255)]); // top half: red
            assert_eq!(hb.bg, vec![pack_rgba(0, 0, 255, 255)]); // bottom half: blue
        }

        #[test]
        fn cell_colors_are_the_integer_mean() {
            let rgba = [
                255, 0, 0, 255, 255, 0, 0, 255, // red
                0, 0, 255, 255, 0, 0, 255, 255, // blue
            ];
            let image = ImageRef::new(2, 2, &rgba).unwrap();
            let grid = Grid::new(2, 2, 1, 1.0);
            let colors = extract_cell_colors(&image, &grid).unwrap();
            // mean of two red + two blue: r=127, g=0, b=127, a=255 (integer division).
            assert_eq!(colors, vec![pack_rgba(127, 0, 127, 255)]);
        }
    }
}

/// Slot 2 (Unit) — grid geometry: the image partitioned into character cells.
pub mod grid {
    /// How an image is partitioned into a grid of character cells.
    ///
    /// Cell boundaries use integer scaling (`col * width / cols`), so every pixel
    /// belongs to exactly one cell and edge cells absorb any remainder — adjacent
    /// cells differ by at most one pixel, with no uncovered strip.
    #[derive(Debug, Clone, Copy)]
    pub struct Grid {
        cols: u32,
        rows: u32,
        width: u32,
        height: u32,
    }

    impl Grid {
        /// Build a grid of `cols` columns whose row count keeps the image
        /// proportional for a character cell `cell_aspect` times taller than wide
        /// (~2.0 for typical monospace fonts). All fields are clamped to at least 1.
        pub fn new(width: u32, height: u32, cols: u32, cell_aspect: f32) -> Grid {
            let cols = cols.max(1);
            let cell_w = width as f32 / cols as f32;
            let cell_h = cell_w * cell_aspect.max(0.01);
            let rows = ((height as f32 / cell_h).round() as u32).max(1);
            Grid {
                cols,
                rows,
                width,
                height,
            }
        }

        pub fn cols(&self) -> u32 {
            self.cols
        }

        pub fn rows(&self) -> u32 {
            self.rows
        }

        /// Pixel bounds `(x0, x1, y0, y1)` of cell `(col, row)`, as half-open
        /// ranges `[x0, x1) × [y0, y1)`. Guaranteed non-empty and in-bounds.
        pub fn cell_bounds(&self, col: u32, row: u32) -> (u32, u32, u32, u32) {
            let x0 = (col as u64 * self.width as u64 / self.cols as u64) as u32;
            let x1 = ((col + 1) as u64 * self.width as u64 / self.cols as u64) as u32;
            let y0 = (row as u64 * self.height as u64 / self.rows as u64) as u32;
            let y1 = ((row + 1) as u64 * self.height as u64 / self.rows as u64) as u32;
            (
                x0,
                x1.max(x0 + 1).min(self.width),
                y0,
                y1.max(y0 + 1).min(self.height),
            )
        }
    }
}

/// Slot 3 (Feature vocabulary) — the ASCII vocabulary and its extraction.
pub mod feature {
    use super::error::Error;
    use super::grid::Grid;
    use super::image::ImageRef;
    use glyph_atlas::{PATCH_COLS, PATCH_ROWS, PATCH_SLOTS};
    use mosaic_core::feature::{FeatureField, FeatureSchema, FeatureType, Gather};

    /// The core per-cell stride — luminance (L0, slot 0), gradient magnitude/orientation
    /// (L1, slots 1–2), normalized cell-centre position `u`/`v` (L0, slots 3–4), and mean
    /// cell colour `r`/`g`/`b` (L0, slots 5–7) — as a compile-time constant, equal to
    /// `vocabulary().total_slots()`; used on the hot path so a schema `Vec` + `String` keys
    /// are not rebuilt every render just to sum a constant (asserted equal in
    /// `vocabulary_matches_core_schema`).
    pub(crate) const CORE_STRIDE: u32 = 8;

    /// Reject a feature buffer whose byte size overflows or exceeds
    /// [`crate::MAX_FEATURE_BYTES`], *before* it is allocated — so a pathological grid
    /// (e.g. a 1×1 image with a huge `cols`) can never drive a multi-GB or aborting
    /// allocation, from any entry point including the `pub` extractors.
    fn check_feature_budget(ncells: usize, stride: u32) -> Result<(), Error> {
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

    /// The declared feature vocabulary:
    /// - `luminance` — L0, self-only scalar (slot 0).
    /// - `gradient` — L1, a radius-1 gathered `Vector{2}` of (magnitude,
    ///   orientation) (slots 1–2).
    /// - `position` — L0, self-only `Vector{2}` of the cell's normalized centre `(u, v)`,
    ///   each in `(0, 1)` (slots 3–4). Resolution-independent, exact, deterministic.
    /// - `color` — L0, self-only `Vector{3}` of the cell's mean colour `(r, g, b)`, each
    ///   normalized to `[0, 1]` (slots 5–7). The same deterministic integer mean the tint /
    ///   half-block render uses, so a Facet reads exactly the colour it would be tinted with.
    ///
    /// L2 (`patch`, sub-cell structure) is appended here when it lands.
    pub fn vocabulary() -> FeatureSchema {
        FeatureSchema {
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
                    key: "position".into(),
                    ty: FeatureType::Vector { len: 2 },
                    gather: Gather::SelfOnly,
                },
                FeatureField {
                    key: "color".into(),
                    ty: FeatureType::Vector { len: 3 },
                    gather: Gather::SelfOnly,
                },
            ],
        }
    }

    /// Per-cell features laid out per [`vocabulary`], row-major over cells, each
    /// cell occupying `stride` (= `schema.total_slots()`) contiguous `f32`s:
    /// `[luminance, gradient_magnitude, gradient_orientation, u, v, r, g, b]`.
    #[derive(Debug, Clone)]
    pub struct FeatureBuffer {
        pub cols: u32,
        pub rows: u32,
        pub stride: u32,
        pub data: Vec<f32>,
    }

    impl FeatureBuffer {
        /// The `stride`-length feature slice for cell `(col, row)`.
        pub fn cell(&self, col: u32, row: u32) -> &[f32] {
            let stride = self.stride as usize;
            let start = ((row as usize * self.cols as usize) + col as usize) * stride;
            &self.data[start..start + stride]
        }
    }

    /// Measure the vocabulary over every cell.
    ///
    /// Pass 1 computes each cell's mean luminance (L0). Pass 2 computes the Sobel
    /// gradient of that luminance grid — each cell gathering its 8 neighbors with
    /// edge-clamping (radius-1 gather, D5/O1) — storing magnitude and orientation.
    pub fn extract(image: &ImageRef, grid: &Grid) -> Result<FeatureBuffer, Error> {
        let stride = CORE_STRIDE;
        let cols = grid.cols();
        let rows = grid.rows();
        let ncells = cols as usize * rows as usize;
        check_feature_budget(ncells, stride)?;

        // Pass 1 — mean luminance per cell.
        let mut luminance = vec![0.0f32; ncells];
        for row in 0..rows {
            for col in 0..cols {
                let (x0, x1, y0, y1) = grid.cell_bounds(col, row);
                let mut sum = 0.0f32;
                let mut count = 0u32;
                for y in y0..y1 {
                    for x in x0..x1 {
                        sum += image.luma(x, y);
                        count += 1;
                    }
                }
                let mean = if count > 0 { sum / count as f32 } else { 0.0 };
                luminance[row as usize * cols as usize + col as usize] = mean;
            }
        }

        // Pass 2 — Sobel gradient over the cell luminance grid.
        let sample = |c: i64, r: i64| -> f32 {
            let cc = c.clamp(0, cols as i64 - 1) as usize;
            let rr = r.clamp(0, rows as i64 - 1) as usize;
            luminance[rr * cols as usize + cc]
        };
        let mut data = vec![0.0f32; ncells * stride as usize];
        for row in 0..rows {
            for col in 0..cols {
                let c = col as i64;
                let r = row as i64;
                let gx = -sample(c - 1, r - 1) + sample(c + 1, r - 1) - 2.0 * sample(c - 1, r)
                    + 2.0 * sample(c + 1, r)
                    - sample(c - 1, r + 1)
                    + sample(c + 1, r + 1);
                let gy = -sample(c - 1, r - 1) - 2.0 * sample(c, r - 1) - sample(c + 1, r - 1)
                    + sample(c - 1, r + 1)
                    + 2.0 * sample(c, r + 1)
                    + sample(c + 1, r + 1);
                let base = (row as usize * cols as usize + col as usize) * stride as usize;
                data[base] = luminance[row as usize * cols as usize + col as usize];
                data[base + 1] = (gx * gx + gy * gy).sqrt();
                data[base + 2] = libm::atan2f(gy, gx);
                // Normalized cell-centre position, each in (0, 1). Cell-centre (not corner)
                // so it is symmetric at both edges and never divides by zero — the loop only
                // runs when `cols`/`rows` >= 1. The cast, the `+ 0.5`, and the divide are each
                // exact or correctly-rounded in f32 (cols/rows < 2^24 here, bounded by the
                // feature budget), so the result is bit-identical native vs wasm (preview == render).
                data[base + 3] = (col as f32 + 0.5) / cols as f32;
                data[base + 4] = (row as f32 + 0.5) / rows as f32;
                // Mean cell colour, normalized per channel to [0, 1]. Reuses the same
                // deterministic integer mean (`color::mean_color`) as the tint / half-block
                // render, so a Facet reads exactly the colour it would be tinted with; the
                // channel is an exact 0..255 integer and `/255.0` is correctly rounded, so it
                // is bit-identical native vs wasm.
                let (mx0, mx1, my0, my1) = grid.cell_bounds(col, row);
                let mean = crate::color::mean_color(image, mx0, mx1, my0, my1);
                data[base + 5] = (mean & 0xff) as f32 / 255.0;
                data[base + 6] = ((mean >> 8) & 0xff) as f32 / 255.0;
                data[base + 7] = ((mean >> 16) & 0xff) as f32 / 255.0;
            }
        }

        Ok(FeatureBuffer {
            cols,
            rows,
            stride,
            data,
        })
    }

    /// The declared **L2 structural** vocabulary: a single self-only
    /// [`FeatureType::Patch`] of sub-cell luminance samples. Separate from
    /// [`vocabulary`] (L0+L1) so a Facet opts into the 64-slot patch only when it
    /// needs it — density/edge Facets never pay for it.
    pub fn vocabulary_structural() -> FeatureSchema {
        FeatureSchema {
            fields: vec![FeatureField {
                key: "patch".into(),
                ty: FeatureType::Patch {
                    rows: PATCH_ROWS as u16,
                    cols: PATCH_COLS as u16,
                },
                gather: Gather::SelfOnly,
            }],
        }
    }

    /// Extract the L2 sub-cell luminance patch for every cell: each cell's pixel
    /// region is downsampled to a `PATCH_ROWS`×`PATCH_COLS` grid of mean luminance
    /// (row-major), the input a Facet shape-matches against the glyph atlas. Sub
    /// blocks smaller than a pixel sample the nearest pixel, so tiny cells still
    /// yield a defined patch (no panic, no division by zero).
    pub fn extract_structural(image: &ImageRef, grid: &Grid) -> Result<FeatureBuffer, Error> {
        let stride = PATCH_SLOTS as u32;
        let cols = grid.cols();
        let rows = grid.rows();
        let ncells = cols as usize * rows as usize;
        check_feature_budget(ncells, stride)?;
        let mut data = vec![0.0f32; ncells * stride as usize];

        for row in 0..rows {
            for col in 0..cols {
                let (x0, x1, y0, y1) = grid.cell_bounds(col, row);
                let cw = (x1 - x0) as u64;
                let ch = (y1 - y0) as u64;
                let base = (row as usize * cols as usize + col as usize) * stride as usize;
                for pr in 0..PATCH_ROWS {
                    for pc in 0..PATCH_COLS {
                        let sx0 = x0 + (pc as u64 * cw / PATCH_COLS as u64) as u32;
                        let sx1 = x0 + ((pc as u64 + 1) * cw / PATCH_COLS as u64) as u32;
                        let sy0 = y0 + (pr as u64 * ch / PATCH_ROWS as u64) as u32;
                        let sy1 = y0 + ((pr as u64 + 1) * ch / PATCH_ROWS as u64) as u32;
                        let val = if sx1 > sx0 && sy1 > sy0 {
                            let mut sum = 0.0f32;
                            let mut count = 0u32;
                            for y in sy0..sy1 {
                                for x in sx0..sx1 {
                                    sum += image.luma(x, y);
                                    count += 1;
                                }
                            }
                            sum / count as f32
                        } else {
                            // Sub-block finer than one pixel: sample the nearest
                            // pixel, clamped inside the cell.
                            let px = sx0.min(x1 - 1);
                            let py = sy0.min(y1 - 1);
                            image.luma(px, py)
                        };
                        data[base + pr * PATCH_COLS + pc] = val;
                    }
                }
            }
        }

        Ok(FeatureBuffer {
            cols,
            rows,
            stride,
            data,
        })
    }
}

/// Slots 4 & 5 (Output primitive + Composition) — the density + edge Facet.
pub mod render {
    use super::error::Error;
    use super::feature::FeatureBuffer;
    use mosaic_core::manifest::{Control, Manifest, Param};

    /// The default glyph ramp, sparse → dense, for dark-background terminals.
    pub const DEFAULT_RAMP: &str = " .:-=+*#%@";

    /// User-facing options for the density + edge Facet.
    #[derive(Debug, Clone)]
    pub struct Options {
        /// Target output width in characters.
        pub cols: u32,
        /// Character cell aspect (height / width); ~2.0 for typical monospace.
        pub cell_aspect: f32,
        /// Ordered glyph ramp, sparse → dense.
        pub ramp: Vec<char>,
        /// If `true`, invert the density mapping (bright → sparse).
        pub invert: bool,
        /// If `true`, draw directional glyphs on cells whose gradient magnitude
        /// exceeds [`Options::edge_threshold`].
        pub edges: bool,
        /// Gradient magnitude above which an edge glyph replaces the density glyph.
        pub edge_threshold: f32,
    }

    impl Default for Options {
        fn default() -> Self {
            Options {
                cols: 100,
                cell_aspect: 2.0,
                ramp: DEFAULT_RAMP.chars().collect(),
                invert: false,
                edges: true,
                edge_threshold: 0.6,
            }
        }
    }

    /// The parameter surface Mosaic would render into controls — the Facet's
    /// [`Manifest`]. Declaring it here validates the manifest model.
    pub fn manifest() -> Manifest {
        Manifest {
            params: vec![
                Param {
                    key: "cols".into(),
                    label: "Columns".into(),
                    help: Some("Output width in characters.".into()),
                    control: Control::Int {
                        default: 100,
                        min: 8,
                        max: 400,
                    },
                },
                Param {
                    key: "ramp".into(),
                    label: "Glyph ramp".into(),
                    help: Some("Ordered glyphs, sparse to dense.".into()),
                    control: Control::Text {
                        default: DEFAULT_RAMP.into(),
                        max_len: 256,
                    },
                },
                Param {
                    key: "invert".into(),
                    label: "Invert".into(),
                    help: Some("Map bright regions to sparse glyphs.".into()),
                    control: Control::Bool { default: false },
                },
                Param {
                    key: "edges".into(),
                    label: "Edge glyphs".into(),
                    help: Some("Draw directional glyphs on strong edges.".into()),
                    control: Control::Bool { default: true },
                },
                Param {
                    key: "edge_threshold".into(),
                    label: "Edge threshold".into(),
                    help: Some("Gradient magnitude above which an edge is drawn.".into()),
                    control: Control::Float {
                        default: 0.6,
                        min: 0.0,
                        max: 4.0,
                        step: Some(0.05),
                    },
                },
            ],
        }
    }

    /// Map a cell's luminance (slot 0) to a glyph via the density ramp.
    fn density_glyph(luma: f32, ramp: &[char], invert: bool) -> char {
        let n = ramp.len();
        let l = if invert { 1.0 - luma } else { luma };
        let l = l.clamp(0.0, 1.0);
        // `+ 0.5) as usize`, not `.round()`: these differ for one f32 luma
        // (0x3D638E38, where l*(n-1) is the exact tie 0.4999999702), and the
        // sandboxed/browser facet-ramp uses this form — so `.round()` here broke the
        // byte-identical native≡Facet contract at that value. Match every other ramp
        // path in the tree (facet-ramp, tessera-spectral, the DSL). Audit M5.
        let idx = (l * (n as f32 - 1.0) + 0.5) as usize;
        ramp[idx.min(n - 1)]
    }

    /// Map a gradient orientation to a line glyph for the edge (perpendicular to
    /// the gradient), quantized to four directions.
    fn edge_glyph(gradient_dir: f32) -> char {
        use core::f32::consts::PI;
        // Edge direction is perpendicular to the gradient; shift by an eighth-turn
        // so the four bins center on 0, π/4, π/2, 3π/4.
        let a = (gradient_dir + PI / 2.0 + PI / 8.0).rem_euclid(PI);
        match (a / (PI / 4.0)) as u32 {
            0 => '-',
            1 => '/',
            2 => '|',
            _ => '\\',
        }
    }

    /// Map one cell's features to a glyph: an edge glyph when the gradient is
    /// strong (and enabled), otherwise a density glyph.
    fn glyph_for_cell(feat: &[f32], opts: &Options) -> char {
        // Defensive: read only what is present, so a caller-built FeatureBuffer with
        // a short stride yields density (never an out-of-bounds panic).
        let luma = feat.first().copied().unwrap_or(0.0);
        let mag = feat.get(1).copied().unwrap_or(0.0);
        let dir = feat.get(2).copied().unwrap_or(0.0);
        if opts.edges && mag > opts.edge_threshold {
            edge_glyph(dir)
        } else {
            density_glyph(luma, &opts.ramp, opts.invert)
        }
    }

    /// Compose per-cell features into an ASCII string (rows separated by `\n`).
    pub fn compose(buf: &FeatureBuffer, opts: &Options) -> Result<String, Error> {
        if opts.ramp.is_empty() {
            return Err(Error::EmptyRamp);
        }
        let mut out = String::with_capacity((buf.cols as usize + 1) * buf.rows as usize);
        for row in 0..buf.rows {
            for col in 0..buf.cols {
                out.push(glyph_for_cell(buf.cell(col, row), opts));
            }
            if row + 1 < buf.rows {
                out.push('\n');
            }
        }
        Ok(out)
    }

    #[cfg(test)]
    mod density_tests {
        use super::density_glyph;

        #[test]
        fn matches_facet_ramp_at_the_f32_tie() {
            // luma bits 0x3D638E38: l*(n-1) is the exact f32 tie 0.4999999702 for
            // n=10, where the old `.round()` picked ' ' but the sandboxed facet-ramp's
            // `+0.5` truncation picks '.'. They must now agree. Audit M5.
            let ramp: Vec<char> = " .:-=+*#%@".chars().collect();
            let luma = f32::from_bits(0x3D63_8E38);
            let n = ramp.len();
            let facet_idx = (luma * (n as f32 - 1.0) + 0.5) as usize;
            assert_eq!(
                density_glyph(luma, &ramp, false),
                ramp[facet_idx.min(n - 1)]
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A solid `width × height` image of one opaque RGB color.
    fn solid(width: u32, height: u32, rgb: (u8, u8, u8)) -> Vec<u8> {
        let (r, g, b) = rgb;
        let mut v = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for _ in 0..(width * height) {
            v.extend_from_slice(&[r, g, b, 255]);
        }
        v
    }

    /// An image whose left half is black and right half white (a vertical edge).
    fn vertical_edge(w: u32, h: u32) -> Vec<u8> {
        let mut v = vec![0u8; w as usize * h as usize * 4];
        for y in 0..h {
            for x in 0..w {
                let val: u8 = if x < w / 2 { 0 } else { 255 };
                let i = (y as usize * w as usize + x as usize) * 4;
                v[i] = val;
                v[i + 1] = val;
                v[i + 2] = val;
                v[i + 3] = 255;
            }
        }
        v
    }

    /// An image whose top half is black and bottom white (a horizontal edge).
    fn horizontal_edge(w: u32, h: u32) -> Vec<u8> {
        let mut v = vec![0u8; w as usize * h as usize * 4];
        for y in 0..h {
            for x in 0..w {
                let val: u8 = if y < h / 2 { 0 } else { 255 };
                let i = (y as usize * w as usize + x as usize) * 4;
                v[i] = val;
                v[i + 1] = val;
                v[i + 2] = val;
                v[i + 3] = 255;
            }
        }
        v
    }

    /// Options with edges disabled, isolating pure L0 density behavior.
    fn density_opts(cols: u32, invert: bool) -> Options {
        Options {
            cols,
            cell_aspect: 1.0,
            ramp: DEFAULT_RAMP.chars().collect(),
            invert,
            edges: false,
            edge_threshold: 0.6,
        }
    }

    #[test]
    fn rejects_mismatched_buffer() {
        let data = vec![0u8; 10];
        assert_eq!(
            ImageRef::new(2, 2, &data).unwrap_err(),
            Error::BufferSizeMismatch {
                expected: 16,
                actual: 10
            }
        );
        let ok = solid(2, 2, (0, 0, 0));
        assert!(ImageRef::new(2, 2, &ok).is_ok());
    }

    #[test]
    fn rejects_empty_image() {
        assert_eq!(ImageRef::new(0, 4, &[]).unwrap_err(), Error::EmptyImage);
    }

    #[test]
    fn solid_black_and_white_hit_ramp_ends() {
        let opts = density_opts(4, false);
        let white = solid(4, 4, (255, 255, 255));
        let out = render_ascii(&ImageRef::new(4, 4, &white).unwrap(), &opts).unwrap();
        assert!(out.chars().filter(|c| *c != '\n').all(|c| c == '@'));

        let black = solid(4, 4, (0, 0, 0));
        let out = render_ascii(&ImageRef::new(4, 4, &black).unwrap(), &opts).unwrap();
        assert!(out.chars().filter(|c| *c != '\n').all(|c| c == ' '));
    }

    #[test]
    fn invert_flips_mapping() {
        let opts = density_opts(2, true);
        let white = solid(2, 2, (255, 255, 255));
        let out = render_ascii(&ImageRef::new(2, 2, &white).unwrap(), &opts).unwrap();
        assert!(out.chars().filter(|c| *c != '\n').all(|c| c == ' '));
    }

    #[test]
    fn output_shape_matches_grid() {
        let data = solid(8, 8, (128, 128, 128));
        let opts = density_opts(4, false);
        let out = render_ascii(&ImageRef::new(8, 8, &data).unwrap(), &opts).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 4);
        for line in &lines {
            assert_eq!(line.chars().count(), 4);
        }
    }

    #[test]
    fn vertical_edge_draws_vertical_glyphs() {
        let data = vertical_edge(8, 8);
        let opts = Options {
            edges: true,
            ..density_opts(4, false)
        };
        let out = render_ascii(&ImageRef::new(8, 8, &data).unwrap(), &opts).unwrap();
        for line in out.lines() {
            assert_eq!(line, " ||@");
        }
    }

    #[test]
    fn horizontal_edge_draws_horizontal_glyphs() {
        let data = horizontal_edge(8, 8);
        let opts = Options {
            edges: true,
            ..density_opts(4, false)
        };
        let out = render_ascii(&ImageRef::new(8, 8, &data).unwrap(), &opts).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines, ["    ", "----", "----", "@@@@"]);
    }

    #[test]
    fn vocabulary_matches_core_schema() {
        let schema = feature::vocabulary();
        // The hot-path constant must stay equal to the declared vocabulary's slot count.
        assert_eq!(schema.total_slots(), feature::CORE_STRIDE);
        assert_eq!(feature::CORE_STRIDE, 8);
        // Position is self-only, so it does not widen the gather radius past the gradient's.
        assert_eq!(schema.max_radius(), 1);
    }

    #[test]
    fn zero_columns_and_empty_ramp_error() {
        let data = solid(2, 2, (0, 0, 0));
        let img = ImageRef::new(2, 2, &data).unwrap();
        let opts = Options {
            cols: 0,
            ..Options::default()
        };
        assert_eq!(render_ascii(&img, &opts), Err(Error::ZeroColumns));
        let opts = Options {
            cols: 4,
            ramp: vec![],
            ..Options::default()
        };
        assert_eq!(render_ascii(&img, &opts), Err(Error::EmptyRamp));
    }

    // --- Property / stress tests ---

    /// Tiny deterministic xorshift PRNG for reproducible randomized sweeps.
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

    #[test]
    fn render_never_panics_and_output_shape_holds() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..300 {
            let w = rng.range(1, 48);
            let h = rng.range(1, 48);
            let data: Vec<u8> = (0..(w * h * 4)).map(|_| rng.next_u32() as u8).collect();
            let img = ImageRef::new(w, h, &data).unwrap();
            let cols = rng.range(1, 80);
            let aspect = 0.5 + (rng.next_u32() % 300) as f32 / 100.0; // 0.5..=3.5
            let opts = Options {
                cols,
                cell_aspect: aspect,
                ramp: DEFAULT_RAMP.chars().collect(),
                invert: rng.next_u32() & 1 == 0,
                edges: rng.next_u32() & 1 == 0,
                edge_threshold: (rng.next_u32() % 400) as f32 / 100.0, // 0..=4
            };
            let out = render_ascii(&img, &opts).unwrap();

            // Output shape exactly matches the grid: `rows` lines of `cols` chars.
            let grid = Grid::new(w, h, cols, aspect);
            let lines: Vec<&str> = out.lines().collect();
            assert_eq!(lines.len() as u32, grid.rows());
            for line in &lines {
                assert_eq!(line.chars().count() as u32, grid.cols());
            }

            // Determinism: rendering again yields identical output.
            assert_eq!(render_ascii(&img, &opts).unwrap(), out);
        }
    }

    #[test]
    fn handles_degenerate_geometry() {
        // 1x1 image renders without panic.
        let one = solid(1, 1, (200, 200, 200));
        assert!(render_ascii(&ImageRef::new(1, 1, &one).unwrap(), &Options::default()).is_ok());

        // Far more columns than pixels wide: every line still has `cols` chars.
        let small = solid(3, 3, (0, 0, 0));
        let opts = Options {
            cols: 100,
            ..Options::default()
        };
        let out = render_ascii(&ImageRef::new(3, 3, &small).unwrap(), &opts).unwrap();
        assert!(out.lines().all(|l| l.chars().count() == 100));
    }

    // --- L2 structural (sub-cell patch) ---

    #[test]
    fn structural_vocabulary_matches_core_schema() {
        let schema = feature::vocabulary_structural();
        assert_eq!(schema.total_slots(), 64); // 8x8 patch
        assert_eq!(schema.max_radius(), 0); // self-only
    }

    #[test]
    fn structural_density_extremes_anchor() {
        // Solid white -> full block; solid black -> space, through the whole L2 path.
        let white = solid(8, 8, (255, 255, 255));
        let out = render_structural(&ImageRef::new(8, 8, &white).unwrap(), 1, 2.0).unwrap();
        assert_eq!(out, "\u{2588}");
        let black = solid(8, 8, (0, 0, 0));
        let out = render_structural(&ImageRef::new(8, 8, &black).unwrap(), 1, 2.0).unwrap();
        assert_eq!(out, " ");
    }

    #[test]
    fn structural_output_shape_and_determinism() {
        let data = solid(16, 16, (128, 128, 128));
        let img = ImageRef::new(16, 16, &data).unwrap();
        let out = render_structural(&img, 4, 1.0).unwrap();
        let grid = Grid::new(16, 16, 4, 1.0);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len() as u32, grid.rows());
        for line in &lines {
            assert_eq!(line.chars().count() as u32, grid.cols());
        }
        // Determinism: same image + params -> identical output.
        assert_eq!(render_structural(&img, 4, 1.0).unwrap(), out);
    }

    #[test]
    fn structural_handles_degenerate_and_zero_cols() {
        // 1x1 image, far more cols than pixels: every patch slot is defined via the
        // nearest-pixel fallback (no panic), and a uniform image yields uniform glyphs.
        let one = solid(1, 1, (200, 200, 200));
        let img = ImageRef::new(1, 1, &one).unwrap();
        let out = render_structural(&img, 20, 2.0).unwrap();
        let first = out.chars().find(|c| *c != '\n').unwrap();
        assert!(out.chars().filter(|c| *c != '\n').all(|c| c == first));
        assert_eq!(render_structural(&img, 0, 2.0), Err(Error::ZeroColumns));
    }

    #[test]
    fn extract_rejects_oversized_grid_instead_of_panicking() {
        // A 1x1 image with an enormous cols and a tiny cell aspect yields a grid of
        // ~10^12 cells: the byte budget must reject both extractors with a clean
        // Error, never a capacity-overflow panic (the crate's no-panic contract).
        let one = solid(1, 1, (128, 128, 128));
        let img = ImageRef::new(1, 1, &one).unwrap();
        let grid = Grid::new(1, 1, 100_000, 0.01);
        assert!(matches!(
            feature::extract(&img, &grid),
            Err(Error::FeatureBufferTooLarge { .. })
        ));
        assert!(matches!(
            feature::extract_structural(&img, &grid),
            Err(Error::FeatureBufferTooLarge { .. })
        ));
    }

    #[test]
    fn position_is_the_normalized_cell_centre() {
        // Every cell's slots 3/4 are its centre (u, v), each strictly in (0, 1), equal to
        // (col+0.5)/cols and (row+0.5)/rows bit-for-bit (the exact f32 divide the extractor
        // does — so the golden cross-check holds native vs wasm).
        let data = solid(8, 8, (128, 128, 128));
        let img = ImageRef::new(8, 8, &data).unwrap();
        let grid = Grid::new(8, 8, 4, 1.0);
        let buf = feature::extract(&img, &grid).unwrap();
        assert_eq!(buf.stride, 8);
        let (cols, rows) = (buf.cols, buf.rows);
        assert!(cols >= 2 && rows >= 2, "want a non-degenerate grid");
        for row in 0..rows {
            for col in 0..cols {
                let cell = buf.cell(col, row);
                let (u, v) = (cell[3], cell[4]);
                assert_eq!(u, (col as f32 + 0.5) / cols as f32);
                assert_eq!(v, (row as f32 + 0.5) / rows as f32);
                assert!(u > 0.0 && u < 1.0 && v > 0.0 && v < 1.0);
            }
        }
        // The top-left cell centre sits half a cell in from the origin, not on it.
        let tl = buf.cell(0, 0);
        assert_eq!(tl[3], 0.5 / cols as f32);
        assert_eq!(tl[4], 0.5 / rows as f32);
    }

    #[test]
    fn color_slots_are_the_normalized_cell_mean() {
        // A spatially-varying image (r ramps with x, g with y, b anti-ramps with x) so cells
        // differ — this proves the extractor reads the correct per-cell region, which a solid
        // image could not. Slots 5/6/7 must equal, bit-for-bit, the same integer mean the tint
        // path (`extract_cell_colors`) produces for that cell, so a Facet reads exactly the
        // colour it would be tinted with.
        let (w, h) = (8u32, 8u32);
        let mut data = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                data[i] = (x * 32) as u8; // r: 0..224 across x
                data[i + 1] = (y * 32) as u8; // g: 0..224 across y
                data[i + 2] = 255 - (x * 32) as u8; // b: 255..31 across x
                data[i + 3] = 255;
            }
        }
        let img = ImageRef::new(w, h, &data).unwrap();
        let grid = Grid::new(w, h, 4, 1.0);
        let buf = feature::extract(&img, &grid).unwrap();
        assert_eq!(buf.stride, 8);
        let tint = color::extract_cell_colors(&img, &grid).unwrap();
        assert_eq!(tint.len(), buf.data.len() / 8);
        let mut distinct = std::collections::BTreeSet::new();
        for (i, cell) in buf.data.chunks_exact(8).enumerate() {
            let packed = tint[i];
            assert_eq!(cell[5], (packed & 0xff) as f32 / 255.0);
            assert_eq!(cell[6], ((packed >> 8) & 0xff) as f32 / 255.0);
            assert_eq!(cell[7], ((packed >> 16) & 0xff) as f32 / 255.0);
            assert!((0.0..=1.0).contains(&cell[5]));
            distinct.insert(packed);
        }
        assert!(
            distinct.len() > 1,
            "cells must differ so the per-cell region is actually exercised"
        );
    }

    #[test]
    fn braille_maps_brightness_to_dots() {
        // Bright sub-cells raise dots, dark ones don't. Solid white -> every dot -> ⣿ (U+28FF);
        // solid black -> no dots -> ⠀ (U+2800). Every glyph stays in the braille block.
        let white = solid(8, 8, (255, 255, 255));
        let out = render_braille(&ImageRef::new(8, 8, &white).unwrap(), 4, 1.0).unwrap();
        assert!(out.chars().filter(|c| *c != '\n').all(|c| c == '\u{28FF}'));

        let black = solid(8, 8, (0, 0, 0));
        let out = render_braille(&ImageRef::new(8, 8, &black).unwrap(), 4, 1.0).unwrap();
        assert!(out.chars().filter(|c| *c != '\n').all(|c| c == '\u{2800}'));
        for line in out.lines() {
            assert!(line.chars().all(|c| ('\u{2800}'..='\u{28FF}').contains(&c)));
        }

        // Bit layout: a 2×4 image is exactly one cell of 1-pixel sub-cells, so lighting one
        // pixel raises exactly one braille dot. Check all eight against the Unicode dot
        // numbering derived *independently* of the engine's `DOT_BITS` table, so a permuted
        // mapping (e.g. swapping dots 7 and 8) can't slip through self-consistently.
        assert_eq!(
            (
                Grid::new(2, 4, 1, 2.0).cols(),
                Grid::new(2, 4, 1, 2.0).rows()
            ),
            (1, 1)
        );
        for sr in 0..4u32 {
            for sc in 0..2u32 {
                // Unicode braille: left column holds dots 1,2,3,7 (bits 0,1,2,6); the right
                // column dots 4,5,6,8 (bits 3,4,5,7).
                let expected_bit: u32 = match (sr, sc) {
                    (0, 0) => 0,
                    (1, 0) => 1,
                    (2, 0) => 2,
                    (3, 0) => 6,
                    (0, 1) => 3,
                    (1, 1) => 4,
                    (2, 1) => 5,
                    _ => 7,
                };
                let mut px = vec![0u8; 2 * 4 * 4];
                let i = ((sr * 2 + sc) * 4) as usize;
                px[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
                let out = render_braille(&ImageRef::new(2, 4, &px).unwrap(), 1, 2.0).unwrap();
                let expected = char::from_u32(0x2800 + (1u32 << expected_bit)).unwrap();
                assert_eq!(
                    out.trim_end_matches('\n').chars().next().unwrap(),
                    expected,
                    "sub-cell (row {sr}, col {sc}) must light braille bit {expected_bit}"
                );
            }
        }

        // Zero columns is a clean error, never a panic.
        assert_eq!(
            render_braille(&ImageRef::new(8, 8, &white).unwrap(), 0, 1.0),
            Err(Error::ZeroColumns)
        );
    }

    // --- Propagation: error-diffusion dithering ---

    #[test]
    fn dither_solid_levels_and_stipple() {
        // Solid white -> all '@'; solid black -> all ' '.
        let white = solid(8, 8, (255, 255, 255));
        let out = render_dither(&ImageRef::new(8, 8, &white).unwrap(), 4, 1.0).unwrap();
        assert!(out.chars().filter(|c| *c != '\n').all(|c| c == '@'));
        let black = solid(8, 8, (0, 0, 0));
        let out = render_dither(&ImageRef::new(8, 8, &black).unwrap(), 4, 1.0).unwrap();
        assert!(out.chars().filter(|c| *c != '\n').all(|c| c == ' '));

        // A flat mid-grey stipples into a mix of both glyphs (impossible with pure
        // gather) and is deterministic.
        let grey = solid(16, 16, (128, 128, 128));
        let img = ImageRef::new(16, 16, &grey).unwrap();
        let out = render_dither(&img, 8, 1.0).unwrap();
        let glyphs: Vec<char> = out.chars().filter(|c| *c != '\n').collect();
        assert!(glyphs.contains(&'@'));
        assert!(glyphs.contains(&' '));
        assert_eq!(render_dither(&img, 8, 1.0).unwrap(), out);
    }
}
