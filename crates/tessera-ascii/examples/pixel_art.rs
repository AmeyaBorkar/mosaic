//! "Pixel-art" demo for `tessera-ascii`: the output primitive is a grid of glyphs, so
//! feeding it a **Unicode block ramp** (` ░ ▒ ▓ █`) turns per-cell density into shaded
//! pixels. Same engine + pipeline as the ASCII demo — only the Facet's ramp changes.
//!
//! Run: `cargo run -p tessera-ascii --example pixel_art`

use tessera_ascii::{ImageRef, Options, render_ascii};

/// Build an opaque grayscale RGBA image from `f(u, v) -> [0,1]`, where (u, v) are
/// normalized coordinates in [-1, 1] with +v pointing up.
fn image(w: u32, h: u32, f: impl Fn(f32, f32) -> f32) -> Vec<u8> {
    let mut buf = vec![0u8; w as usize * h as usize * 4];
    for y in 0..h {
        for x in 0..w {
            let u = (x as f32 / (w as f32 - 1.0)) * 2.0 - 1.0;
            let v = 1.0 - (y as f32 / (h as f32 - 1.0)) * 2.0;
            let g = (f(u, v).clamp(0.0, 1.0) * 255.0).round() as u8;
            let i = (y as usize * w as usize + x as usize) * 4;
            buf[i] = g;
            buf[i + 1] = g;
            buf[i + 2] = g;
            buf[i + 3] = 255;
        }
    }
    buf
}

fn main() {
    // The whole "make it pixel art" knob: a 5-step block ramp instead of the default
    // character ramp, and edges off (pure density, no directional line glyphs).
    let opts = Options {
        cols: 44,
        ramp: " ░▒▓█".chars().collect(),
        edges: false,
        cell_aspect: 2.0,
        ..Options::default()
    };

    let (w, h) = (256u32, 256u32);

    // A heart: the classic implicit curve (x²+y²−1)³ − x²·y³ ≤ 0, softened to a gradient.
    let heart = image(w, h, |u, v| {
        let (u, v) = (u * 1.25, v * 1.25 + 0.2);
        let a = u * u + v * v - 1.0;
        let f = a * a * a - u * u * v * v * v; // < 0 inside the heart
        (0.5 - f * 6.0).clamp(0.0, 1.0)
    });
    println!("input: 256x256 grayscale heart  ->  render_ascii, 44 cols, block ramp\n");
    println!(
        "{}\n",
        render_ascii(&ImageRef::new(w, h, &heart).unwrap(), &opts).unwrap()
    );

    // A radial disk with a soft edge — reads as a shaded sphere.
    let disk = image(w, h, |u, v| 1.0 - (u * u + v * v).sqrt().min(1.0));
    println!("input: 256x256 radial disk  ->  block ramp\n");
    println!(
        "{}",
        render_ascii(&ImageRef::new(w, h, &disk).unwrap(), &opts).unwrap()
    );
}
