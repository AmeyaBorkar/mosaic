//! Emit the **composition (O4) browser-conformance golden**.
//!
//! Builds genuine cross-engine layer data — an image ASCII render and an audio
//! spectrogram, each produced by the *sandboxed* `facet-ramp` — then composites them with
//! `mosaic_core::composite` and records both the layers (tokens + coverage + placement)
//! and the native composed text. `crates/mosaic-wasm/test/composite.test.ts` rebuilds each
//! composition through the browser `Canvas` binding from the stored layers and must
//! reproduce the text byte-for-byte, proving browser composition == native. (Engine
//! extract determinism is proven separately by the render/spectral goldens; this isolates
//! the compositor.)
//!
//! Deterministic: signals use `libm::sinf`, images use integer ramps, and the sandbox is
//! fixed — so the golden reproduces byte-for-byte and `verify-fixtures.sh` stays green.
//!
//! Run: `cargo run -p mosaic-wasm --example emit_composite_golden`

use std::fs;
use std::path::Path;

use mosaic_core::composite::{Blend, Canvas, Layer};
use mosaic_runtime::{Facet, Limits, Sandbox};
use tessera_ascii::{Grid, ImageRef, feature as ascii_feature};
use tessera_spectral::{SignalRef, SpectroGrid, feature as spectral_feature};

const FACET_RAMP: &[u8] = include_bytes!("../tests/facet_ramp.wasm");
const SPACE: u32 = b' ' as u32;
const DOT: u32 = b'.' as u32;
const TAU: f32 = core::f32::consts::TAU;

/// One layer, ready to serialize and to place.
struct LayerSpec {
    cols: u32,
    rows: u32,
    tokens: Vec<u32>,
    coverage: Vec<f32>,
    row_off: i32,
    col_off: i32,
    blend: Blend,
}

struct Case {
    name: &'static str,
    canvas_cols: u32,
    canvas_rows: u32,
    background: u32,
    layers: Vec<LayerSpec>,
}

fn main() {
    let sandbox = Sandbox::new().expect("sandbox");
    let facet = sandbox.compile(FACET_RAMP).expect("compile facet-ramp");

    let (ic, ir, itokens) = image_tokens(&sandbox, &facet);
    let (sc, sr, stokens, senergy) = spectral_tokens(&sandbox, &facet);

    let mut cases = Vec::new();

    // 1. Cross-engine stack: image render on top, spectrogram below, in one canvas.
    let image_cov: Vec<f32> = keyed_coverage(&itokens, SPACE);
    let spectro_cov: Vec<f32> = keyed_coverage(&stokens, SPACE);
    cases.push(Case {
        name: "cross_engine_stack",
        canvas_cols: ic.max(sc),
        canvas_rows: ir + sr,
        background: SPACE,
        layers: vec![
            LayerSpec {
                cols: ic,
                rows: ir,
                tokens: itokens.clone(),
                coverage: image_cov,
                row_off: 0,
                col_off: 0,
                blend: Blend::Over,
            },
            LayerSpec {
                cols: sc,
                rows: sr,
                tokens: stokens.clone(),
                coverage: spectro_cov,
                row_off: ir as i32,
                col_off: 0,
                blend: Blend::Over,
            },
        ],
    });

    // 2. Spectrogram stippled by band energy: the energy itself is the coverage, so
    //    StippleOver dithers the glyphs in by intensity over a '.' background.
    cases.push(Case {
        name: "spectrogram_stippled_by_energy",
        canvas_cols: sc,
        canvas_rows: sr,
        background: DOT,
        layers: vec![LayerSpec {
            cols: sc,
            rows: sr,
            tokens: stokens.clone(),
            coverage: senergy.clone(),
            row_off: 0,
            col_off: 0,
            blend: Blend::StippleOver,
        }],
    });

    let json = render_json(&cases);
    let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("test");
    fs::create_dir_all(&out_dir).expect("create test dir");
    fs::write(out_dir.join("composite_golden.json"), &json).expect("write composite_golden.json");
    println!(
        "emit_composite_golden: wrote {} cases to {}",
        cases.len(),
        out_dir.display()
    );
}

/// Image ASCII tokens via the sandboxed facet-ramp over a diagonal-gradient image.
fn image_tokens(sandbox: &Sandbox, facet: &Facet) -> (u32, u32, Vec<u32>) {
    let (w, h, cols) = (48u32, 24u32, 24u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let v = (((x + y) * 255) / (w + h - 2)) as u8;
            let i = ((y * w + x) * 4) as usize;
            rgba[i] = v;
            rgba[i + 1] = v;
            rgba[i + 2] = v;
            rgba[i + 3] = 255;
        }
    }
    let img = ImageRef::new(w, h, &rgba).unwrap();
    let grid = Grid::new(w, h, cols, 2.0);
    let feats = ascii_feature::extract(&img, &grid).unwrap();
    let ncells = (feats.cols * feats.rows) as usize;
    let tokens = sandbox
        .run_map(
            facet,
            Limits::default(),
            &feats.data,
            ncells,
            feats.stride as usize,
        )
        .unwrap();
    (feats.cols, feats.rows, tokens)
}

/// Spectrogram tokens (via sandboxed facet-ramp) and the raw band energy, from a chirp
/// sized so its frame count matches the image width.
fn spectral_tokens(sandbox: &Sandbox, facet: &Facet) -> (u32, u32, Vec<u32>, Vec<f32>) {
    let (sr, bands, win, hop, n) = (8000u32, 12u32, 256u32, 128u32, 3200usize);
    let (fmin, fmax) = (120.0f32, 3600.0f32);
    // Linear chirp fmin -> fmax via a phase accumulator (libm::sinf, deterministic).
    let mut phase = 0.0f32;
    let mut samples = Vec::with_capacity(n);
    for i in 0..n {
        let f = fmin + (fmax - fmin) * (i as f32 / n as f32);
        samples.push(libm::sinf(phase));
        phase += TAU * f / sr as f32;
    }
    let sig = SignalRef::new(&samples, sr).unwrap();
    let grid = SpectroGrid::new(bands, win, hop, fmin, fmax);
    let buf = spectral_feature::extract(&sig, &grid).unwrap();
    let ncells = (buf.cols * buf.rows) as usize;
    let tokens = sandbox
        .run_map(
            facet,
            Limits::default(),
            &buf.data,
            ncells,
            buf.stride as usize,
        )
        .unwrap();
    (buf.cols, buf.rows, tokens, buf.data)
}

fn keyed_coverage(tokens: &[u32], background: u32) -> Vec<f32> {
    tokens
        .iter()
        .map(|&t| if t == background { 0.0 } else { 1.0 })
        .collect()
}

fn render_json(cases: &[Case]) -> String {
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str(
        "  \"note\": \"AUTO-GENERATED by `cargo run -p mosaic-wasm --example emit_composite_golden`. Do not edit by hand.\",\n",
    );
    json.push_str("  \"cases\": [\n");
    for (ci, case) in cases.iter().enumerate() {
        // Compose natively to record the authoritative text.
        let mut canvas = Canvas::new(case.canvas_cols, case.canvas_rows).unwrap();
        for l in &case.layers {
            let layer =
                Layer::with_coverage(l.cols, l.rows, l.tokens.clone(), l.coverage.clone()).unwrap();
            canvas.place(&layer, l.row_off, l.col_off, l.blend);
        }
        let text = canvas.into_text(case.background);

        json.push_str("    {\n");
        json.push_str(&format!("      \"name\": {:?},\n", case.name));
        json.push_str(&format!(
            "      \"canvas\": {{ \"cols\": {}, \"rows\": {} }},\n",
            case.canvas_cols, case.canvas_rows
        ));
        json.push_str(&format!("      \"background\": {},\n", case.background));
        json.push_str("      \"layers\": [\n");
        for (li, l) in case.layers.iter().enumerate() {
            json.push_str("        {\n");
            json.push_str(&format!(
                "          \"cols\": {}, \"rows\": {}, \"rowOff\": {}, \"colOff\": {}, \"blend\": {:?},\n",
                l.cols, l.rows, l.row_off, l.col_off, blend_name(l.blend)
            ));
            json.push_str(&format!(
                "          \"tokens\": [{}],\n",
                join_u32(&l.tokens)
            ));
            json.push_str(&format!(
                "          \"coverage\": [{}]\n",
                join_f32(&l.coverage)
            ));
            json.push_str(if li + 1 == case.layers.len() {
                "        }\n"
            } else {
                "        },\n"
            });
        }
        json.push_str("      ],\n");
        json.push_str(&format!("      \"text\": \"{}\"\n", json_escape(&text)));
        json.push_str(if ci + 1 == cases.len() {
            "    }\n"
        } else {
            "    },\n"
        });
    }
    json.push_str("  ]\n}\n");
    json
}

fn blend_name(b: Blend) -> &'static str {
    match b {
        Blend::Over => "over",
        Blend::Under => "under",
        Blend::Replace => "replace",
        Blend::StippleOver => "stipple",
    }
}

fn join_u32(v: &[u32]) -> String {
    v.iter()
        .map(|x| x.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn join_f32(v: &[f32]) -> String {
    v.iter()
        .map(|x| format!("{x}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn json_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}
