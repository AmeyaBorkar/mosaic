//! Shared Floyd–Steinberg error-diffusion for the ASCII engine's **propagation**
//! method (decision D5) — the one genuinely sequential, feedback-driven method class
//! the parallel gather model cannot express.
//!
//! A cell is quantized to one of two levels; the quantization *error* is diffused to
//! not-yet-processed neighbours along the classic Floyd–Steinberg kernel, so a region
//! of flat grey renders as a stippled mix of the two glyphs (1-bit dithering) rather
//! than a single glyph. The engine owns the traversal order; the Facet is a pure,
//! sandboxed transform.
//!
//! This crate is `no_std` and dependency-free so the **exact same** routine compiles
//! into both the native engine (`tessera-ascii`) and the untrusted wasm Facet
//! (`facets/dither`). There is therefore one implementation, not two that could
//! drift, and — raster order + exact power-of-two weights + no transcendentals —
//! it is bit-identical on every target, so the preview cannot diverge from the
//! render.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

/// Output codepoint for the dark level (empty). On a dark-background terminal a
/// bright cell carries ink, so the bright level is [`BRIGHT`].
pub const DARK: u32 = 0x20; // ' '
/// Output codepoint for the bright level (full).
pub const BRIGHT: u32 = 0x40; // '@'

// Floyd–Steinberg kernel weights (denominator 16 = 2^4, so each is an exact f32).
const W_RIGHT: f32 = 7.0 / 16.0;
const W_DOWN_LEFT: f32 = 3.0 / 16.0;
const W_DOWN: f32 = 5.0 / 16.0;
const W_DOWN_RIGHT: f32 = 1.0 / 16.0;

/// 1-bit Floyd–Steinberg dither of the per-cell luminance — slot 0 of each cell's
/// `stride` features — writing one output codepoint per cell into `out`.
///
/// `features` is modified in place: each cell's quantization error is diffused into
/// later cells' luminance slots. Processing is raster order and every neighbour
/// update uses an exact power-of-two weight, so the accumulation into any cell occurs
/// in a fixed sequence and the result is bit-identical across the native and wasm
/// targets. Out-of-range inputs are handled without panic (the host guarantees the
/// lengths, but a short slice simply returns).
pub fn floyd_steinberg(
    features: &mut [f32],
    cols: usize,
    rows: usize,
    stride: usize,
    out: &mut [u32],
) {
    if stride == 0 {
        return;
    }
    let ncells = cols.saturating_mul(rows);
    if features.len() < ncells.saturating_mul(stride) || out.len() < ncells {
        return;
    }

    for row in 0..rows {
        for col in 0..cols {
            let idx = row * cols + col;
            let value = features[idx * stride];
            let level = if value >= 0.5 { 1.0f32 } else { 0.0f32 };
            out[idx] = if level == 1.0 { BRIGHT } else { DARK };
            let error = value - level;

            // Diffuse to not-yet-processed neighbours (right, then the row below).
            if col + 1 < cols {
                features[(idx + 1) * stride] += error * W_RIGHT;
            }
            if row + 1 < rows {
                let below = idx + cols;
                if col > 0 {
                    features[(below - 1) * stride] += error * W_DOWN_LEFT;
                }
                features[below * stride] += error * W_DOWN;
                if col + 1 < cols {
                    features[(below + 1) * stride] += error * W_DOWN_RIGHT;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_levels_map_to_single_glyph() {
        // All-bright -> all '@' (error 0); all-dark -> all ' '.
        let mut white = [1.0f32; 6];
        let mut out = [0u32; 6];
        floyd_steinberg(&mut white, 3, 2, 1, &mut out);
        assert!(out.iter().all(|&c| c == BRIGHT));

        let mut black = [0.0f32; 6];
        floyd_steinberg(&mut black, 3, 2, 1, &mut out);
        assert!(out.iter().all(|&c| c == DARK));
    }

    #[test]
    fn flat_grey_stipples() {
        // A single 0.5 grey row dithers: first cell rounds up to '@', its -0.5 error
        // diffuses (7/16) to pull the next cell below threshold -> ' '.
        let mut grey = [0.5f32, 0.5];
        let mut out = [0u32; 2];
        floyd_steinberg(&mut grey, 2, 1, 1, &mut out);
        assert_eq!(out, [BRIGHT, DARK]);
    }

    #[test]
    fn respects_stride_reading_only_luminance() {
        // stride 3 (luma, mag, dir): only slot 0 is read/diffused; the others are
        // untouched noise. Two bright luma cells -> two '@'.
        let mut feats = [1.0, 9.9, -9.9, 1.0, 9.9, -9.9];
        let mut out = [0u32; 2];
        floyd_steinberg(&mut feats, 2, 1, 3, &mut out);
        assert_eq!(out, [BRIGHT, BRIGHT]);
    }

    #[test]
    fn deterministic_and_panic_free_on_short_slices() {
        let mut a = [0.3, 0.7, 0.55, 0.1, 0.9, 0.42];
        let mut oa = [0u32; 6];
        floyd_steinberg(&mut a, 3, 2, 1, &mut oa);
        let mut b = [0.3, 0.7, 0.55, 0.1, 0.9, 0.42];
        let mut ob = [0u32; 6];
        floyd_steinberg(&mut b, 3, 2, 1, &mut ob);
        assert_eq!(oa, ob);

        // Too-short buffers just return, no panic.
        let mut short = [0.5f32; 2];
        let mut oshort = [0u32; 1];
        floyd_steinberg(&mut short, 3, 3, 1, &mut oshort);
    }
}
