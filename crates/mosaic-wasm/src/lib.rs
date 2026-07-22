//! wasm-bindgen browser bindings for the Mosaic engine.
//!
//! This crate compiles the **same** engines (`tessera-ascii` and `tessera-spectral`)
//! to `wasm32` so the browser runs identical feature-extraction and composition to the
//! native/server path — one implementation, no drift (decision D2). It deliberately
//! does **not** depend on `mosaic-runtime` (wasmtime is native-only); untrusted Facets
//! execute in the browser via `@mosaic/facet-abi` on the browser's own WebAssembly
//! engine (decision D9).
//!
//! The browser render pipeline is three steps:
//! 1. [`extract_features`] — image → per-cell feature buffer (here, wasm).
//! 2. the Facet — feature buffer → per-cell `u32` tokens (`@mosaic/facet-abi`).
//! 3. [`compose`] — tokens → validated ASCII text (here, wasm).

use mosaic_core::composite;
use tessera_ascii::feature;
use tessera_ascii::{Grid, ImageRef, MAX_CELLS};
use tessera_spectral::{SignalRef, SpectroGrid, feature as spectral_feature};
use wasm_bindgen::prelude::*;

/// Per-cell feature buffer produced by [`extract_features`]: `cols * rows` cells,
/// each `stride` little-endian `f32`s, ready to hand to a Facet.
#[wasm_bindgen]
pub struct FeatureBuffer {
    cols: u32,
    rows: u32,
    stride: u32,
    data: Vec<f32>,
}

#[wasm_bindgen]
impl FeatureBuffer {
    /// Grid columns (output width in characters).
    #[wasm_bindgen(getter)]
    pub fn cols(&self) -> u32 {
        self.cols
    }

    /// Grid rows (output height in characters).
    #[wasm_bindgen(getter)]
    pub fn rows(&self) -> u32 {
        self.rows
    }

    /// Feature slots per cell (for ASCII L0+L1, `3`: luminance, grad magnitude,
    /// grad orientation).
    #[wasm_bindgen(getter)]
    pub fn stride(&self) -> u32 {
        self.stride
    }

    /// Number of cells (`cols * rows`) — the Facet's `ncells` argument.
    #[wasm_bindgen(getter)]
    pub fn ncells(&self) -> u32 {
        self.cols.saturating_mul(self.rows)
    }

    /// A zero-copy `Float32Array` **view** of the feature values — the Facet's input.
    ///
    /// The view aliases this module's wasm linear memory, so it must be consumed
    /// (copied out, or handed to the Facet host, which copies it into the guest
    /// immediately) before any subsequent call into this module — a memory growth
    /// would detach it. Avoids cloning the whole buffer on every read.
    #[wasm_bindgen(getter)]
    pub fn data(&self) -> js_sys::Float32Array {
        // SAFETY: the returned view is copied by the caller before any wasm allocation
        // (facet-abi marshals it into the guest immediately), so it never outlives the
        // buffer it borrows.
        unsafe { js_sys::Float32Array::view(&self.data) }
    }
}

/// Shared grid + guard logic for both vocabularies; `extractor` is the native
/// per-cell measurement (`feature::extract` for L0+L1, `feature::extract_structural`
/// for L2). `rgba` is row-major 8-bit RGBA (4 bytes/pixel), the layout of a canvas
/// `ImageData`. Throws on a mismatched buffer size, zero columns, or a grid
/// exceeding [`MAX_CELLS`].
fn extract_with(
    rgba: &[u8],
    width: u32,
    height: u32,
    cols: u32,
    cell_aspect: f32,
    extractor: fn(&ImageRef, &Grid) -> Result<feature::FeatureBuffer, tessera_ascii::Error>,
) -> Result<FeatureBuffer, JsError> {
    if cols == 0 {
        return Err(JsError::new("cols must be greater than zero"));
    }
    let image = ImageRef::new(width, height, rgba).map_err(|e| JsError::new(&e.to_string()))?;
    let grid = Grid::new(width, height, cols, cell_aspect);
    let ncells = (grid.cols() as usize)
        .checked_mul(grid.rows() as usize)
        .ok_or_else(|| JsError::new("grid dimensions overflow"))?;
    if ncells > MAX_CELLS {
        return Err(JsError::new(&format!(
            "grid has {ncells} cells, exceeding the maximum of {MAX_CELLS}"
        )));
    }
    let buf = extractor(&image, &grid).map_err(|e| JsError::new(&e.to_string()))?;
    Ok(FeatureBuffer {
        cols: buf.cols,
        rows: buf.rows,
        stride: buf.stride,
        data: buf.data,
    })
}

/// Extract the **L0+L1** vocabulary (luminance + gradient, stride 3) — the density
/// and edge features. For the density/edge Facet (`facet_ramp`).
#[wasm_bindgen]
pub fn extract_features(
    rgba: &[u8],
    width: u32,
    height: u32,
    cols: u32,
    cell_aspect: f32,
) -> Result<FeatureBuffer, JsError> {
    extract_with(rgba, width, height, cols, cell_aspect, feature::extract)
}

/// Extract the **L2** vocabulary (an 8×8 sub-cell luminance patch, stride 64) — the
/// structural feature. For the glyph-matching Facet (`facet_structural`).
#[wasm_bindgen]
pub fn extract_structural_features(
    rgba: &[u8],
    width: u32,
    height: u32,
    cols: u32,
    cell_aspect: f32,
) -> Result<FeatureBuffer, JsError> {
    extract_with(
        rgba,
        width,
        height,
        cols,
        cell_aspect,
        feature::extract_structural,
    )
}

/// Extract the **spectral** vocabulary (per-cell band energy, stride 1) from a block of
/// mono PCM `samples` — the second engine's features. `samples` is a `Float32Array` of
/// audio; the grid is `bands` log-spaced frequency rows measured over a `win`-sample
/// window hopped by `hop`, covering `[fmin, fmax]` Hz. The buffer is fed to a scalar
/// gather Facet (e.g. `facet_ramp`) exactly like the image features — the same
/// domain-agnostic Facet, now on the browser path for audio too.
///
/// Throws on an empty signal, a zero sample rate, an out-of-range band spec, or a grid
/// exceeding the engine's cell budget.
#[wasm_bindgen]
pub fn extract_spectral_features(
    samples: &[f32],
    sample_rate: u32,
    bands: u32,
    win: u32,
    hop: u32,
    fmin: f32,
    fmax: f32,
) -> Result<FeatureBuffer, JsError> {
    let signal = SignalRef::new(samples, sample_rate).map_err(|e| JsError::new(&e.to_string()))?;
    let grid = SpectroGrid::new(bands, win, hop, fmin, fmax);
    let buf =
        spectral_feature::extract(&signal, &grid).map_err(|e| JsError::new(&e.to_string()))?;
    Ok(FeatureBuffer {
        cols: buf.cols,
        rows: buf.rows,
        stride: buf.stride,
        data: buf.data,
    })
}

/// Compose per-cell output tokens (`u32` codepoints from a Facet) into ASCII text,
/// row-major with `\n` between rows. Invalid codepoints become `U+FFFD` — untrusted
/// Facet output is never assumed valid. This is the single, shared-with-the-server
/// composition, so a malicious token is validated by one implementation, not two.
#[wasm_bindgen]
pub fn compose(cols: u32, rows: u32, codepoints: &[u32]) -> String {
    tessera_ascii::compose_codepoints(cols, rows, codepoints)
}

/// A composition surface for the browser — the wasm-bindgen face of
/// [`mosaic_core::composite::Canvas`] (O4). Build it, [`Canvas::place`] rendered layers
/// (each a token grid plus per-cell coverage) with a blend mode and offset, then
/// [`Canvas::into_text`]. Composition runs host-side on already-produced tokens — from the
/// same engine or different ones — and `into_text` routes every cell through the shared
/// composer, so a composed artifact inherits the untrusted-glyph boundary exactly as a
/// single render does.
#[wasm_bindgen]
pub struct Canvas {
    inner: composite::Canvas,
}

#[wasm_bindgen]
impl Canvas {
    /// A new, fully transparent `cols × rows` canvas. Throws on a zero or oversized grid.
    #[wasm_bindgen(constructor)]
    pub fn new(cols: u32, rows: u32) -> Result<Canvas, JsError> {
        let inner = composite::Canvas::new(cols, rows).map_err(|e| JsError::new(&e.to_string()))?;
        Ok(Canvas { inner })
    }

    /// Composite a layer onto the canvas with its top-left at (`row_off`, `col_off`) and
    /// `blend` — one of `"over"`, `"under"`, `"replace"`, `"stipple"`. `tokens` and
    /// `coverage` are the layer's `layer_cols × layer_rows` grid, row-major, coverage in
    /// `[0, 1]`. Throws on a bad blend name or a size mismatch.
    // A flat argument list is the natural shape for this wasm-bindgen boundary method;
    // bundling into a struct would only move the ceremony into JS.
    #[allow(clippy::too_many_arguments)]
    pub fn place(
        &mut self,
        tokens: &[u32],
        coverage: &[f32],
        layer_cols: u32,
        layer_rows: u32,
        row_off: i32,
        col_off: i32,
        blend: &str,
    ) -> Result<(), JsError> {
        let blend = parse_blend(blend)?;
        let layer = composite::Layer::with_coverage(
            layer_cols,
            layer_rows,
            tokens.to_vec(),
            coverage.to_vec(),
        )
        .map_err(|e| JsError::new(&e.to_string()))?;
        self.inner.place(&layer, row_off, col_off, blend);
        Ok(())
    }

    /// Resolve the canvas to text: transparent cells become `background`, then the shared
    /// composer validates every glyph. Consumes the canvas.
    pub fn into_text(self, background: u32) -> String {
        self.inner.into_text(background)
    }
}

fn parse_blend(name: &str) -> Result<composite::Blend, JsError> {
    match name {
        "over" => Ok(composite::Blend::Over),
        "under" => Ok(composite::Blend::Under),
        "replace" => Ok(composite::Blend::Replace),
        "stipple" => Ok(composite::Blend::StippleOver),
        other => Err(JsError::new(&format!("unknown blend mode: {other:?}"))),
    }
}
