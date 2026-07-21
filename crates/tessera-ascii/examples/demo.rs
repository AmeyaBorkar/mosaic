//! Visual smoke test for `tessera-ascii`: renders synthetic images to ASCII.
//!
//! Run with: `cargo run -p tessera-ascii --example demo`

use tessera_ascii::{ImageRef, Options, render_ascii};

/// Build an opaque RGBA buffer from a grayscale function `f(x, y) -> [0, 1]`.
fn gray_image(w: u32, h: u32, f: impl Fn(u32, u32) -> f32) -> Vec<u8> {
    let mut buf = vec![0u8; w as usize * h as usize * 4];
    for y in 0..h {
        for x in 0..w {
            let v = (f(x, y).clamp(0.0, 1.0) * 255.0).round() as u8;
            let i = (y as usize * w as usize + x as usize) * 4;
            buf[i] = v;
            buf[i + 1] = v;
            buf[i + 2] = v;
            buf[i + 3] = 255;
        }
    }
    buf
}

fn show(title: &str, img: &ImageRef, opts: &Options) {
    println!("{title}:\n");
    println!("{}\n", render_ascii(img, opts).unwrap());
}

fn main() {
    let opts = Options {
        cols: 60,
        ..Options::default()
    };

    // 1) Horizontal gradient — smooth, so it stays on the density ramp.
    let (w, h) = (200, 100);
    let buf = gray_image(w, h, |x, _| x as f32 / (w as f32 - 1.0));
    show(
        &format!("Horizontal gradient ({w}x{h} px)"),
        &ImageRef::new(w, h, &buf).unwrap(),
        &opts,
    );

    // 2) Radial disk — bright core fading to dark edges.
    let (w, h) = (240, 240);
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let buf = gray_image(w, h, |x, y| {
        let dx = x as f32 - cx;
        let dy = y as f32 - cy;
        1.0 - ((dx * dx + dy * dy).sqrt() / (w as f32 * 0.42)).min(1.0)
    });
    show(
        &format!("Radial disk ({w}x{h} px)"),
        &ImageRef::new(w, h, &buf).unwrap(),
        &opts,
    );

    // 3) Filled rectangle — hard edges showcase L1 directional glyphs.
    let (w, h) = (240, 160);
    let buf = gray_image(w, h, |x, y| {
        let inside = x > w / 4 && x < 3 * w / 4 && y > h / 4 && y < 3 * h / 4;
        if inside { 1.0 } else { 0.0 }
    });
    show(
        &format!("Filled rectangle ({w}x{h} px) — edge glyphs"),
        &ImageRef::new(w, h, &buf).unwrap(),
        &opts,
    );
}
