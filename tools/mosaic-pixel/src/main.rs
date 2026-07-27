//! `mosaic-pixel` — a standalone demo: turn any image into colorful **hexagonal**
//! pixel-art PNGs.
//!
//! Not part of the Mosaic platform pipeline (no engine/Facet/sandbox) — it does its own
//! decode, downsample, color styling, and hex rasterization natively, for quickly making
//! art you can look at. The platform tie-in is conceptual: the same "one styled color per
//! cell" idea the Facet ABI expresses (a `u32` per cell), rendered as hexes instead of
//! glyphs.
//!
//! Usage:
//!   mosaic-pixel <input-image> [OPTIONS]
//!
//! Options:
//!   --out-dir <DIR>    output folder (created if missing; default: ./pixel-art-output)
//!   --cols <N>         horizontal cell count = resolution (default: 96)
//!   --cell <PX>        output pixels per hex (size); default: 16
//!   --style <S>        truecolor | posterize | palette | dither | all   (default: all)
//!   --palette <P>      pico8 | gameboy | cga   (for palette/dither; default: pico8)
//!   --levels <N>       posterize levels per channel (default: 4)
//!
//! Example:
//!   mosaic-pixel surfer.png --cols 120 --cell 18 --style all --palette pico8

use std::path::PathBuf;
use std::process::ExitCode;

use image::{Rgba, RgbaImage};

#[derive(Clone, Copy, Debug)]
struct Color {
    r: f32,
    g: f32,
    b: f32,
}

impl Color {
    fn to_rgba(self) -> Rgba<u8> {
        let q = |v: f32| v.round().clamp(0.0, 255.0) as u8;
        Rgba([q(self.r), q(self.g), q(self.b), 255])
    }
    fn dist2(self, o: [u8; 3]) -> f32 {
        let (dr, dg, db) = (
            self.r - o[0] as f32,
            self.g - o[1] as f32,
            self.b - o[2] as f32,
        );
        dr * dr + dg * dg + db * db
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    TrueColor,
    Posterize,
    Palette,
    Dither,
}

impl Style {
    fn name(self) -> &'static str {
        match self {
            Style::TrueColor => "truecolor",
            Style::Posterize => "posterize",
            Style::Palette => "palette",
            Style::Dither => "dither",
        }
    }
}

// --- Palettes (classic, for the palette + dither styles) ---

const PICO8: &[[u8; 3]] = &[
    [0, 0, 0],
    [29, 43, 83],
    [126, 37, 83],
    [0, 135, 81],
    [171, 82, 54],
    [95, 87, 79],
    [194, 195, 199],
    [255, 241, 232],
    [255, 0, 77],
    [255, 163, 0],
    [255, 236, 39],
    [0, 228, 54],
    [41, 173, 255],
    [131, 118, 156],
    [255, 119, 168],
    [255, 204, 170],
];

const GAMEBOY: &[[u8; 3]] = &[[15, 56, 15], [48, 98, 48], [139, 172, 15], [155, 188, 15]];

const CGA: &[[u8; 3]] = &[
    [0, 0, 0],
    [0, 0, 170],
    [0, 170, 0],
    [0, 170, 170],
    [170, 0, 0],
    [170, 0, 170],
    [170, 85, 0],
    [170, 170, 170],
    [85, 85, 85],
    [85, 85, 255],
    [85, 255, 85],
    [85, 255, 255],
    [255, 85, 85],
    [255, 85, 255],
    [255, 255, 85],
    [255, 255, 255],
];

struct Config {
    input: PathBuf,
    out_dir: PathBuf,
    cols: usize,
    cell: f32,
    styles: Vec<Style>,
    palette: &'static [[u8; 3]],
    levels: u8,
}

fn parse_args() -> Result<Config, String> {
    let mut input: Option<PathBuf> = None;
    let mut out_dir = PathBuf::from("pixel-art-output");
    let mut cols = 96usize;
    let mut cell = 16.0f32;
    let mut style = "all".to_string();
    let mut palette = "pico8".to_string();
    let mut levels = 4u8;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].clone();
        match a.as_str() {
            "--out-dir" | "--cols" | "--cell" | "--style" | "--palette" | "--levels" => {
                let v = args
                    .get(i + 1)
                    .cloned()
                    .ok_or_else(|| format!("{a} needs a value"))?;
                match a.as_str() {
                    "--out-dir" => out_dir = PathBuf::from(v),
                    "--cols" => {
                        cols = v
                            .parse()
                            .map_err(|_| "--cols must be an integer".to_string())?
                    }
                    "--cell" => {
                        cell = v
                            .parse()
                            .map_err(|_| "--cell must be a number".to_string())?
                    }
                    "--style" => style = v,
                    "--palette" => palette = v,
                    "--levels" => {
                        levels = v
                            .parse()
                            .map_err(|_| "--levels must be an integer".to_string())?
                    }
                    _ => unreachable!(),
                }
                i += 2;
            }
            "-h" | "--help" => return Err("help".into()),
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            _ => {
                input = Some(PathBuf::from(a));
                i += 1;
            }
        }
    }

    let input = input.ok_or("no input image given")?;
    if cols == 0 {
        return Err("--cols must be > 0".into());
    }
    if !(1.0..=256.0).contains(&cell) {
        return Err("--cell must be in 1..=256".into());
    }
    let styles = match style.as_str() {
        "all" => vec![
            Style::TrueColor,
            Style::Posterize,
            Style::Palette,
            Style::Dither,
        ],
        "truecolor" => vec![Style::TrueColor],
        "posterize" => vec![Style::Posterize],
        "palette" => vec![Style::Palette],
        "dither" => vec![Style::Dither],
        s => return Err(format!("unknown style {s}")),
    };
    let palette: &[[u8; 3]] = match palette.as_str() {
        "pico8" => PICO8,
        "gameboy" => GAMEBOY,
        "cga" => CGA,
        p => return Err(format!("unknown palette {p}")),
    };
    if levels < 2 {
        return Err("--levels must be >= 2".into());
    }

    Ok(Config {
        input,
        out_dir,
        cols,
        cell,
        styles,
        palette,
        levels,
    })
}

/// Nearest palette color to `c` (Euclidean in RGB).
fn nearest(c: Color, pal: &[[u8; 3]]) -> Color {
    let best = pal
        .iter()
        .min_by(|a, b| c.dist2(**a).total_cmp(&c.dist2(**b)))
        .copied()
        .unwrap_or([0, 0, 0]);
    Color {
        r: best[0] as f32,
        g: best[1] as f32,
        b: best[2] as f32,
    }
}

fn posterize_channel(v: f32, levels: u8) -> f32 {
    let l = (levels - 1) as f32;
    (v / 255.0 * l).round() / l * 255.0
}

/// Apply a style to the whole cell grid (row-major, `cols*rows`).
fn apply_style(grid: &[Color], cols: usize, rows: usize, cfg: &Config, style: Style) -> Vec<Color> {
    match style {
        Style::TrueColor => grid.to_vec(),
        Style::Posterize => grid
            .iter()
            .map(|c| Color {
                r: posterize_channel(c.r, cfg.levels),
                g: posterize_channel(c.g, cfg.levels),
                b: posterize_channel(c.b, cfg.levels),
            })
            .collect(),
        Style::Palette => grid.iter().map(|c| nearest(*c, cfg.palette)).collect(),
        Style::Dither => dither(grid, cols, rows, cfg.palette),
    }
}

/// Floyd–Steinberg error diffusion toward the palette — textured retro shading.
fn dither(grid: &[Color], cols: usize, rows: usize, pal: &[[u8; 3]]) -> Vec<Color> {
    let mut work = grid.to_vec();
    let mut out = vec![
        Color {
            r: 0.0,
            g: 0.0,
            b: 0.0
        };
        cols * rows
    ];
    for y in 0..rows {
        for x in 0..cols {
            let i = y * cols + x;
            let old = work[i];
            let new = nearest(old, pal);
            out[i] = new;
            let (er, eg, eb) = (old.r - new.r, old.g - new.g, old.b - new.b);
            let mut spread = |xx: usize, yy: usize, f: f32| {
                let j = yy * cols + xx;
                work[j].r += er * f;
                work[j].g += eg * f;
                work[j].b += eb * f;
            };
            if x + 1 < cols {
                spread(x + 1, y, 7.0 / 16.0);
            }
            if y + 1 < rows {
                if x > 0 {
                    spread(x - 1, y + 1, 3.0 / 16.0);
                }
                spread(x, y + 1, 5.0 / 16.0);
                if x + 1 < cols {
                    spread(x + 1, y + 1, 1.0 / 16.0);
                }
            }
        }
    }
    out
}

/// Rasterize a `cols*rows` color grid into a pointy-top hexagon tiling. Every output
/// pixel is painted with the color of the hex whose center is nearest — a hex Voronoi,
/// so cells are gap-free regular hexagons.
fn hex_raster(grid: &[Color], cols: usize, rows: usize, size: f32) -> RgbaImage {
    let sqrt3 = 3.0_f32.sqrt();
    let hstep = sqrt3 * size; // horizontal center-to-center
    let vstep = 1.5 * size; // vertical center-to-center
    let pad = size;

    let center = |col: usize, row: usize| -> (f32, f32) {
        let cx = pad + hstep * (col as f32 + 0.5 + 0.5 * (row & 1) as f32);
        let cy = pad + size + vstep * row as f32;
        (cx, cy)
    };

    let width = (pad * 2.0 + hstep * (cols as f32 + 1.0)).ceil() as u32;
    let height = (pad * 2.0 + size + vstep * rows as f32).ceil() as u32;
    let mut img = RgbaImage::from_pixel(width, height, Rgba([0, 0, 0, 255]));

    for py in 0..height {
        let fy = py as f32 + 0.5;
        let arow = (((fy - pad - size) / vstep).round() as isize).clamp(-1, rows as isize);
        for px in 0..width {
            let fx = px as f32 + 0.5;
            let mut best = f32::INFINITY;
            let mut chosen: Option<usize> = None;
            for dr in -1..=1 {
                let r = arow + dr;
                if r < 0 || r as usize >= rows {
                    continue;
                }
                let ru = r as usize;
                let acol = ((fx - pad) / hstep - 0.5 - 0.5 * (ru & 1) as f32).round() as isize;
                for dc in -1..=1 {
                    let c = acol + dc;
                    if c < 0 || c as usize >= cols {
                        continue;
                    }
                    let cu = c as usize;
                    let (cx, cy) = center(cu, ru);
                    let d = (cx - fx).powi(2) + (cy - fy).powi(2);
                    if d < best {
                        best = d;
                        chosen = Some(ru * cols + cu);
                    }
                }
            }
            if let Some(idx) = chosen {
                img.put_pixel(px, py, grid[idx].to_rgba());
            }
        }
    }
    img
}

fn run(cfg: &Config) -> Result<Vec<PathBuf>, String> {
    let src = image::open(&cfg.input)
        .map_err(|e| format!("could not open {}: {e}", cfg.input.display()))?
        .to_rgba8();
    let (w0, h0) = (src.width(), src.height());

    // Rows chosen to preserve the image's aspect under the hex geometry.
    let cols = cfg.cols;
    let aspect = h0 as f32 / w0 as f32;
    let rows = ((cols as f32) * aspect * (3.0_f32.sqrt() / 1.5))
        .round()
        .max(1.0) as usize;

    // Downsample to one averaged color per cell.
    let small = image::imageops::resize(
        &src,
        cols as u32,
        rows as u32,
        image::imageops::FilterType::Triangle,
    );
    let grid: Vec<Color> = small
        .pixels()
        .map(|p| Color {
            r: p[0] as f32,
            g: p[1] as f32,
            b: p[2] as f32,
        })
        .collect();

    std::fs::create_dir_all(&cfg.out_dir)
        .map_err(|e| format!("could not create {}: {e}", cfg.out_dir.display()))?;
    let stem = cfg
        .input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image");

    let mut written = Vec::new();
    for &style in &cfg.styles {
        let styled = apply_style(&grid, cols, rows, cfg, style);
        let img = hex_raster(&styled, cols, rows, cfg.cell);
        let name = format!("{stem}_hex_{}_{cols}x{rows}.png", style.name());
        let path = cfg.out_dir.join(name);
        img.save(&path)
            .map_err(|e| format!("could not write {}: {e}", path.display()))?;
        written.push(path);
    }
    Ok(written)
}

fn usage() {
    eprintln!(
        "mosaic-pixel <input-image> [--out-dir DIR] [--cols N] [--cell PX] \
         [--style truecolor|posterize|palette|dither|all] [--palette pico8|gameboy|cga] \
         [--levels N]"
    );
}

fn main() -> ExitCode {
    let cfg = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            if e != "help" {
                eprintln!("error: {e}\n");
            }
            usage();
            return if e == "help" {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(2)
            };
        }
    };
    match run(&cfg) {
        Ok(paths) => {
            println!(
                "Saved {} image(s) to {}:",
                paths.len(),
                cfg.out_dir.display()
            );
            for p in &paths {
                println!("  {}", p.display());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
