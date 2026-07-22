//! Shared glyph atlas and sub-cell matcher for the ASCII engine's **L2 structural**
//! method (decision D6).
//!
//! Each cell is reduced by the engine to an [`PATCH_ROWS`]×[`PATCH_COLS`] luminance
//! patch (values in `[0, 1]`, brighter = larger). [`match_glyph`] picks the atlas
//! glyph whose ink pattern is closest to that patch by sum-of-squared-differences.
//! On a dark-background terminal, a brighter region carries more ink, so a bright
//! patch matches a denser glyph and a dark patch matches a sparse one — density and
//! structure fall out of the same nearest-glyph rule.
//!
//! This crate is `no_std` and dependency-free so the **exact same** atlas and matcher
//! compile into both the native engine (`tessera-ascii`) and the untrusted wasm
//! Facet (`facets/structural`). There is therefore no possibility of the preview
//! diverging from the render because two implementations drifted — there is only one.
//!
//! The matcher uses only `f32` add/sub/mul (no transcendentals, no `fma`
//! contraction), so it is bit-identical across the native and wasm targets.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

/// Sub-cell patch height (rows), in samples.
pub const PATCH_ROWS: usize = 8;
/// Sub-cell patch width (columns), in samples.
pub const PATCH_COLS: usize = 8;
/// Total `f32` slots in one patch — the L2 feature stride.
pub const PATCH_SLOTS: usize = PATCH_ROWS * PATCH_COLS;

/// One atlas entry: a Unicode codepoint and its 8×8 ink bitmap. Each `bits[row]`
/// byte is a row, bit `0x80` = leftmost column (col 0), bit `0x01` = col 7.
pub struct Glyph {
    pub codepoint: u32,
    pub bits: [u8; PATCH_ROWS],
}

/// The candidate glyphs, ordered sparse → structured → dense. `SPACE` is first and
/// `FULL_BLOCK` last so the two density extremes anchor the set; ties (a perfectly
/// flat mid-grey patch is equidistant from every glyph) resolve to the earliest
/// entry deterministically.
pub const ATLAS: &[Glyph] = &[
    // ' ' — empty (darkest).
    Glyph {
        codepoint: 0x20,
        bits: [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
    },
    // '\'' — top tick.
    Glyph {
        codepoint: 0x27,
        bits: [0x18, 0x18, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
    },
    // '.' — bottom dot.
    Glyph {
        codepoint: 0x2E,
        bits: [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18],
    },
    // ':' — two dots.
    Glyph {
        codepoint: 0x3A,
        bits: [0x00, 0x18, 0x18, 0x00, 0x00, 0x18, 0x18, 0x00],
    },
    // '_' — bottom bar.
    Glyph {
        codepoint: 0x5F,
        bits: [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF],
    },
    // '-' — mid bar.
    Glyph {
        codepoint: 0x2D,
        bits: [0x00, 0x00, 0x00, 0xFF, 0xFF, 0x00, 0x00, 0x00],
    },
    // '|' — vertical bar.
    Glyph {
        codepoint: 0x7C,
        bits: [0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18],
    },
    // '/' — forward diagonal.
    Glyph {
        codepoint: 0x2F,
        bits: [0x03, 0x06, 0x0C, 0x18, 0x30, 0x60, 0xC0, 0x80],
    },
    // '\\' — back diagonal.
    Glyph {
        codepoint: 0x5C,
        bits: [0xC0, 0x60, 0x30, 0x18, 0x0C, 0x06, 0x03, 0x01],
    },
    // '+' — cross.
    Glyph {
        codepoint: 0x2B,
        bits: [0x18, 0x18, 0x18, 0xFF, 0xFF, 0x18, 0x18, 0x18],
    },
    // 'x' — both diagonals.
    Glyph {
        codepoint: 0x78,
        bits: [0xC3, 0x66, 0x3C, 0x18, 0x3C, 0x66, 0xC3, 0x81],
    },
    // 'o' — small ring.
    Glyph {
        codepoint: 0x6F,
        bits: [0x00, 0x00, 0x3C, 0x66, 0x66, 0x3C, 0x00, 0x00],
    },
    // 'O' — large ring.
    Glyph {
        codepoint: 0x4F,
        bits: [0x3C, 0x66, 0xC3, 0xC3, 0xC3, 0xC3, 0x66, 0x3C],
    },
    // '#' — hatch.
    Glyph {
        codepoint: 0x23,
        bits: [0x24, 0x24, 0xFF, 0x24, 0x24, 0xFF, 0x24, 0x24],
    },
    // '@' — dense blob.
    Glyph {
        codepoint: 0x40,
        bits: [0x7E, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7E],
    },
    // '█' U+2588 — full block (brightest).
    Glyph {
        codepoint: 0x2588,
        bits: [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
    },
];

/// Precomputed ink patches (`0.0`/`1.0`) for every atlas glyph, materialized once at
/// compile time so [`match_glyph`] reads two contiguous arrays instead of extracting a
/// bit per sample every cell. `0.0`/`1.0` are exactly representable and the SSD
/// arithmetic is unchanged, so tokens stay bit-identical — do **not** reassociate or
/// vectorize the reduction, which would change rounding and break native/wasm parity.
static INK: [[f32; PATCH_SLOTS]; ATLAS.len()] = build_ink();

const fn build_ink() -> [[f32; PATCH_SLOTS]; ATLAS.len()] {
    let mut table = [[0.0f32; PATCH_SLOTS]; ATLAS.len()];
    let mut g = 0;
    while g < ATLAS.len() {
        let mut row = 0;
        while row < PATCH_ROWS {
            let mut col = 0;
            while col < PATCH_COLS {
                let bit = (ATLAS[g].bits[row] >> (7 - col)) & 1;
                table[g][row * PATCH_COLS + col] = if bit == 1 { 1.0 } else { 0.0 };
                col += 1;
            }
            row += 1;
        }
        g += 1;
    }
    table
}

/// Return the codepoint of the atlas glyph closest to `patch` (a row-major
/// `PATCH_ROWS`×`PATCH_COLS` luminance patch in `[0, 1]`) by sum of squared
/// differences. A shorter patch is treated as zero-padded (no panic on malformed
/// input, mirroring the engine's defensive style). Ties resolve to the earliest
/// atlas entry, identically on every target.
pub fn match_glyph(patch: &[f32]) -> u32 {
    // Zero-pad a short patch once, so the inner SSD loop carries no per-sample bounds
    // check (the length check is hoisted out of the hot loop).
    let mut buf = [0.0f32; PATCH_SLOTS];
    let n = if patch.len() < PATCH_SLOTS {
        patch.len()
    } else {
        PATCH_SLOTS
    };
    buf[..n].copy_from_slice(&patch[..n]);

    let mut best_cp = ATLAS[0].codepoint;
    let mut best_ssd = f32::INFINITY;
    let mut g = 0;
    while g < ATLAS.len() {
        let ink = &INK[g];
        let mut ssd = 0.0f32;
        let mut i = 0;
        while i < PATCH_SLOTS {
            let d = buf[i] - ink[i];
            ssd += d * d;
            i += 1;
        }
        if ssd < best_ssd {
            best_ssd = ssd;
            best_cp = ATLAS[g].codepoint;
        }
        g += 1;
    }
    best_cp
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a patch from a per-sample closure.
    fn patch(f: impl Fn(usize, usize) -> f32) -> [f32; PATCH_SLOTS] {
        let mut p = [0.0f32; PATCH_SLOTS];
        for row in 0..PATCH_ROWS {
            for col in 0..PATCH_COLS {
                p[row * PATCH_COLS + col] = f(row, col);
            }
        }
        p
    }

    #[test]
    fn density_extremes_map_to_anchors() {
        // All dark -> space; all bright -> full block (both exact, SSD 0).
        assert_eq!(match_glyph(&patch(|_, _| 0.0)), 0x20);
        assert_eq!(match_glyph(&patch(|_, _| 1.0)), 0x2588);
    }

    #[test]
    fn structure_selects_matching_glyph() {
        // A vertical bar in cols 3-4 -> '|'.
        let vbar = patch(|_, col| if col == 3 || col == 4 { 1.0 } else { 0.0 });
        assert_eq!(match_glyph(&vbar), 0x7C);
        // A horizontal bar in rows 3-4 -> '-'.
        let hbar = patch(|row, _| if row == 3 || row == 4 { 1.0 } else { 0.0 });
        assert_eq!(match_glyph(&hbar), 0x2D);
        // A forward diagonal -> '/'.
        let fwd = patch(|row, col| if col == 7 - row { 1.0 } else { 0.0 });
        assert_eq!(match_glyph(&fwd), 0x2F);
    }

    #[test]
    fn flat_midtone_ties_break_to_first_entry() {
        // A perfectly flat 0.5 patch is equidistant from every glyph; the earliest
        // atlas entry (space) wins, deterministically.
        assert_eq!(match_glyph(&patch(|_, _| 0.5)), 0x20);
    }

    #[test]
    fn short_patch_is_zero_padded_not_a_panic() {
        // Fewer than PATCH_SLOTS values: missing samples read as 0.0 (dark).
        assert_eq!(match_glyph(&[]), 0x20);
        assert_eq!(match_glyph(&[1.0, 1.0, 1.0]), 0x20);
    }

    #[test]
    fn every_codepoint_is_a_valid_char() {
        for g in ATLAS {
            assert!(
                char::from_u32(g.codepoint).is_some(),
                "bad codepoint {:#x}",
                g.codepoint
            );
        }
    }
}
