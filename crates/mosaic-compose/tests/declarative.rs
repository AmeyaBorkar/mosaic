//! End-to-end O4.1: a **declarative** cross-engine Composition rendered through a *real*
//! resolver — the image and audio engines plus the sandboxed `facet-ramp`. Proves the
//! declarative layer drives genuine engines to produce one artifact, that it survives a
//! JSON round-trip unchanged (the shareable-format guarantee), and that it is
//! deterministic and non-degenerate (both engines contribute).

use mosaic_compose::{Composition, LayerResolver, LayerSource, RenderError, ResolvedLayer, render};
use mosaic_runtime::{Facet, Limits, Sandbox};
use tessera_ascii::{Grid, ImageRef, feature as ascii_feature};
use tessera_spectral::{SignalRef, SpectroGrid, feature as spectral_feature};

const FACET_RAMP: &[u8] = include_bytes!("facet_ramp.wasm");

/// A concrete resolver: dispatches on `engine`/`input`, runs the engine's extractor and the
/// sandboxed `facet-ramp`, returns the token grid. This is the seam the registry/server
/// fills in production; here it holds fixed inputs.
struct EngineResolver {
    sandbox: Sandbox,
    ramp: Facet,
    image: (u32, u32, Vec<u8>),
    audio: (u32, Vec<f32>),
}

impl EngineResolver {
    fn new() -> Self {
        let sandbox = Sandbox::new().unwrap();
        let ramp = sandbox.compile(FACET_RAMP).unwrap();
        // Diagonal-gradient image.
        let (w, h) = (48u32, 24u32);
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
        // Deterministic broadband noise (fills many bands).
        let mut state: u64 = 0x00C0_FFEE_5EED_0007;
        let audio: Vec<f32> = (0..4096)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                ((state >> 40) as f32 / 0xFF_FFFF as f32) - 0.5
            })
            .collect();
        EngineResolver {
            sandbox,
            ramp,
            image: (w, h, rgba),
            audio: (8000, audio),
        }
    }

    fn run(&self, features: &[f32], ncells: usize, stride: usize) -> Result<Vec<u32>, RenderError> {
        self.sandbox
            .run_map(&self.ramp, Limits::default(), features, ncells, stride)
            .map_err(|e| RenderError::Resolve(e.to_string()))
    }
}

impl LayerResolver for EngineResolver {
    fn resolve(&mut self, source: &LayerSource) -> Result<ResolvedLayer, RenderError> {
        match (source.engine.as_str(), source.input.as_str()) {
            ("ascii", "img") => {
                let (w, h, rgba) = &self.image;
                let img =
                    ImageRef::new(*w, *h, rgba).map_err(|e| RenderError::Resolve(e.to_string()))?;
                let grid = Grid::new(*w, *h, 24, 2.0);
                let feats = ascii_feature::extract(&img, &grid)
                    .map_err(|e| RenderError::Resolve(e.to_string()))?;
                let ncells = (feats.cols * feats.rows) as usize;
                let tokens = self.run(&feats.data, ncells, feats.stride as usize)?;
                Ok(ResolvedLayer {
                    cols: feats.cols,
                    rows: feats.rows,
                    tokens,
                    coverage: None,
                })
            }
            ("spectral", "audio") => {
                let (sr, samples) = &self.audio;
                let sig = SignalRef::new(samples, *sr)
                    .map_err(|e| RenderError::Resolve(e.to_string()))?;
                let grid = SpectroGrid::new(12, 256, 128, 120.0, 3600.0);
                let buf = spectral_feature::extract(&sig, &grid)
                    .map_err(|e| RenderError::Resolve(e.to_string()))?;
                let ncells = (buf.cols * buf.rows) as usize;
                let tokens = self.run(&buf.data, ncells, buf.stride as usize)?;
                Ok(ResolvedLayer {
                    cols: buf.cols,
                    rows: buf.rows,
                    tokens,
                    coverage: None,
                })
            }
            (e, i) => Err(RenderError::Resolve(format!(
                "no source for engine {e:?} input {i:?}"
            ))),
        }
    }
}

/// A cross-engine stack: image ASCII on top, audio spectrogram below, keyed on space.
fn stack_composition() -> Composition {
    let json = r#"
    {
      "canvas": { "cols": 40, "rows": 20 },
      "background": 32,
      "layers": [
        {
          "source": { "engine": "ascii", "facet": "ramp", "input": "img" },
          "at": { "row": 0, "col": 0 },
          "blend": "over",
          "coverage": { "keyed_on": { "token": 32 } }
        },
        {
          "source": { "engine": "spectral", "facet": "ramp", "input": "audio" },
          "at": { "row": 7, "col": 0 },
          "blend": "over",
          "coverage": { "keyed_on": { "token": 32 } }
        }
      ]
    }"#;
    Composition::from_json(json).expect("valid composition JSON")
}

#[test]
fn declarative_cross_engine_render_is_nondegenerate_and_deterministic() {
    let comp = stack_composition();
    let mut resolver = EngineResolver::new();

    let text = render(&comp, &mut resolver).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 20, "canvas height");
    for l in &lines {
        assert_eq!(l.chars().count(), 40, "canvas width");
    }
    // Image occupies the top band (rows 0..6), spectrogram the lower band (rows 7..).
    let top_ink = lines[0..6].iter().any(|l| l.chars().any(|c| c != ' '));
    let bottom_ink = lines[7..].iter().any(|l| l.chars().any(|c| c != ' '));
    assert!(
        top_ink && bottom_ink,
        "both engines must contribute to the declarative artifact"
    );

    // Deterministic across a fresh resolver.
    let mut resolver2 = EngineResolver::new();
    assert_eq!(render(&comp, &mut resolver2).unwrap(), text);
}

#[test]
fn a_composition_renders_identically_after_a_json_round_trip() {
    let comp = stack_composition();
    let round_tripped = Composition::from_json(&comp.to_json().unwrap()).unwrap();
    assert_eq!(comp, round_tripped, "schema survives serialization");

    let mut r1 = EngineResolver::new();
    let mut r2 = EngineResolver::new();
    assert_eq!(
        render(&comp, &mut r1).unwrap(),
        render(&round_tripped, &mut r2).unwrap(),
        "the shared/serialized form renders the same artifact"
    );
}
