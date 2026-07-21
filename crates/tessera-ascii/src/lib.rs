//! # tessera-ascii
//!
//! Mosaic's first **Tessera**: images → ASCII text.
//!
//! Vertical slice proving the pipeline end-to-end and validating the
//! [`mosaic_core`] contract against a real domain (O5):
//!
//! ```text
//! RGBA buffer → grid of cells → features (L0 luminance, L1 gradient) → map → text
//! ```
//!
//! - **L0** — per-cell mean luminance → a density ramp.
//! - **L1** — a Sobel gradient over the cell luminance grid (each cell reading its
//!   8 neighbors: a **radius-1 gather**, the first concrete exercise of decision
//!   D5/O1) → directional glyphs (`- / | \`) on strong edges.
//!
//! The vocabulary is declared as a [`mosaic_core::feature::FeatureSchema`] and
//! features are laid out in that schema's buffer, so adding L2 (sub-cell
//! structure) is additive. The Facet's parameters are a
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

/// Upper bound on grid cells, guarding against pathological allocations from
/// absurd column counts. Generous: 8M cells is far beyond any real ASCII render.
pub const MAX_CELLS: usize = 8_000_000;

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
    let buf = feature::extract(image, &grid);
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
    let buf = feature::extract_structural(image, &grid);
    let stride = buf.stride as usize;
    let mut codepoints = Vec::with_capacity(cells);
    for i in 0..cells {
        let start = i * stride;
        codepoints.push(glyph_atlas::match_glyph(&buf.data[start..start + stride]));
    }
    Ok(compose_codepoints(buf.cols, buf.rows, &codepoints))
}

/// Compose per-cell output codepoints (produced by a Facet) into an ASCII string,
/// row-major with `\n` between rows.
///
/// Untrusted Facet output is never assumed to be safe text. A codepoint is replaced
/// with `U+FFFD` when it is not a valid Unicode scalar (`char::from_u32` fails) **or**
/// when it is unsafe to emit — a C0/C1 control (including `ESC`, which would inject
/// terminal escape sequences, and `LF`/`CR`, which would break the row/column grid)
/// or a bidi/format override used for visual spoofing. Only printable glyphs cross
/// the boundary; the row separators are the sole `\n` this function emits.
pub fn compose_codepoints(cols: u32, rows: u32, codepoints: &[u32]) -> String {
    // Bounded capacity hint: callers must pass sane dimensions (the pipeline caps
    // them at `MAX_CELLS`); never pre-allocate unboundedly from untrusted sizes.
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

/// Whether a scalar value is unsafe to emit from untrusted Facet output and must be
/// masked to `U+FFFD`: any C0/C1 control or `DEL` (`char::is_control`, covering `ESC`,
/// `LF`, `CR`, …) and the bidirectional/format overrides used for visual spoofing.
/// Rust's std cannot query the `Cf` general category without a Unicode table, so the
/// well-known spoofing overrides are listed explicitly.
fn is_unsafe_glyph(c: char) -> bool {
    c.is_control()
        || matches!(c,
            '\u{200E}' | '\u{200F}' | '\u{061C}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}')
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
    use super::grid::Grid;
    use super::image::ImageRef;
    use glyph_atlas::{PATCH_COLS, PATCH_ROWS};
    use mosaic_core::feature::{FeatureField, FeatureSchema, FeatureType, Gather};

    /// The declared feature vocabulary:
    /// - `luminance` — L0, self-only scalar (slot 0).
    /// - `gradient` — L1, a radius-1 gathered `Vector{2}` of (magnitude,
    ///   orientation) (slots 1–2).
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
            ],
        }
    }

    /// Per-cell features laid out per [`vocabulary`], row-major over cells, each
    /// cell occupying `stride` (= `schema.total_slots()`) contiguous `f32`s:
    /// `[luminance, gradient_magnitude, gradient_orientation]`.
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
    pub fn extract(image: &ImageRef, grid: &Grid) -> FeatureBuffer {
        let stride = vocabulary().total_slots();
        let cols = grid.cols();
        let rows = grid.rows();
        let ncells = cols as usize * rows as usize;

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
            }
        }

        FeatureBuffer {
            cols,
            rows,
            stride,
            data,
        }
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
    pub fn extract_structural(image: &ImageRef, grid: &Grid) -> FeatureBuffer {
        let stride = vocabulary_structural().total_slots();
        let cols = grid.cols();
        let rows = grid.rows();
        let ncells = cols as usize * rows as usize;
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

        FeatureBuffer {
            cols,
            rows,
            stride,
            data,
        }
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
        let idx = (l * (n as f32 - 1.0)).round() as usize;
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
        assert_eq!(schema.total_slots(), 3);
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

    #[test]
    fn compose_codepoints_validates_untrusted_output() {
        // 'A', a surrogate (invalid), out-of-range (invalid), 'B' -> replacements.
        let cps = vec![0x41u32, 0xD800, 0x11_0000, 0x42];
        assert_eq!(compose_codepoints(2, 2, &cps), "A\u{FFFD}\n\u{FFFD}B");
        // Too few codepoints: missing cells become the replacement char, no panic.
        assert_eq!(compose_codepoints(2, 1, &[0x41]), "A\u{FFFD}");
    }

    #[test]
    fn compose_codepoints_masks_control_and_bidi() {
        // ESC, LF, DEL, and a RLO bidi override are all unsafe -> U+FFFD; 'A' survives.
        // This blocks terminal-escape injection and newline-driven grid corruption
        // from untrusted Facet output (validated once, for native and browser alike).
        let cps = vec![0x1Bu32, 0x0A, 0x7F, 0x202E, 0x41];
        assert_eq!(
            compose_codepoints(5, 1, &cps),
            "\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}A"
        );
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
}
