//! # mosaic-compose
//!
//! **Declarative, shareable Compositions (O4.1).**
//!
//! [`mosaic_core::composite`] (O4) is the *imperative* compositor — build a `Canvas`,
//! `place` layers in code. This crate is the *declarative* layer above it: a
//! [`Composition`] is pure, serializable data — a canvas plus an ordered stack of layers,
//! each naming the engine + Facet + input that produces it and how it is placed and
//! blended. It serializes to JSON, so a Composition is a first-class artifact the registry
//! can store and share and the web shell can render, exactly like a Facet.
//!
//! The one thing a Composition cannot carry is *how to run an engine* — that is the host's
//! job. [`render`] takes a [`LayerResolver`] (the seam the registry / server fills):
//! given a layer's [`LayerSource`], the resolver produces its token grid, and `render`
//! composites the stack through the O4 primitive. So the schema here is engine-agnostic
//! and depends on nothing but the substrate; the concrete resolver (which knows `ascii`,
//! `spectral`, …) lives in the host.
//!
//! Rendering inherits every O4 guarantee: it runs no untrusted code (the resolver's job is
//! only to *produce tokens*, e.g. by running a Facet in the sandbox), and the final text
//! passes through the shared composer's untrusted-glyph boundary.

#![forbid(unsafe_code)]

use mosaic_core::composite::{Blend, Canvas, Layer};
use serde::{Deserialize, Serialize};

/// A declarative composition: a canvas and an ordered stack of layers (painter's order,
/// first drawn first / bottom-most). Serializes to JSON as the shareable artifact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Composition {
    pub canvas: CanvasSpec,
    /// Codepoint painted where no layer is opaque (e.g. `32` for space).
    pub background: u32,
    pub layers: Vec<LayerDecl>,
}

/// Output grid size of the composed artifact, in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanvasSpec {
    pub cols: u32,
    pub rows: u32,
}

/// One layer in the stack: what produces it, where it sits, how it blends, and how its
/// coverage (transparency) is derived.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerDecl {
    pub source: LayerSource,
    #[serde(default)]
    pub at: Placement,
    pub blend: BlendSpec,
    #[serde(default)]
    pub coverage: CoverageMode,
}

/// What renders a layer: an engine, a Facet on it, the named input to run over, and the
/// Facet's parameters. The host [`LayerResolver`] interprets these; the schema stays
/// engine-agnostic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerSource {
    /// Engine name, e.g. `"ascii"`, `"spectral"`.
    pub engine: String,
    /// Facet name, e.g. `"ramp"`.
    pub facet: String,
    /// Name of the input (image, audio, …) this layer renders. Resolved by the host.
    pub input: String,
    /// Facet parameters as a free-form object; the resolver validates them per Facet.
    #[serde(default)]
    pub params: serde_json::Value,
}

/// Top-left placement of a layer on the canvas (may be negative or off-canvas; the
/// compositor clips).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Placement {
    #[serde(default)]
    pub row: i32,
    #[serde(default)]
    pub col: i32,
}

/// Serializable mirror of [`mosaic_core::composite::Blend`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlendSpec {
    Over,
    Under,
    Replace,
    Stipple,
}

impl From<BlendSpec> for Blend {
    fn from(b: BlendSpec) -> Blend {
        match b {
            BlendSpec::Over => Blend::Over,
            BlendSpec::Under => Blend::Under,
            BlendSpec::Replace => Blend::Replace,
            BlendSpec::Stipple => Blend::StippleOver,
        }
    }
}

/// How a resolved layer's per-cell coverage is derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageMode {
    /// Every cell is opaque.
    #[default]
    Opaque,
    /// Cells equal to `token` are transparent (the common "space is see-through" case).
    KeyedOn { token: u32 },
    /// Use the coverage the resolver supplied (e.g. a soft mask from a feature).
    Explicit,
}

/// A layer's rendered tokens, produced by a [`LayerResolver`]. `coverage` is required only
/// for [`CoverageMode::Explicit`].
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedLayer {
    pub cols: u32,
    pub rows: u32,
    pub tokens: Vec<u32>,
    pub coverage: Option<Vec<f32>>,
}

/// The host seam: turn a [`LayerSource`] into its rendered token grid. This is where a
/// Facet actually runs (e.g. in the sandbox) — the registry / server provides the concrete
/// implementation, keeping this crate engine-agnostic.
pub trait LayerResolver {
    fn resolve(&mut self, source: &LayerSource) -> Result<ResolvedLayer, RenderError>;
}

/// Everything that can go wrong rendering a [`Composition`].
#[derive(Debug, Clone, PartialEq)]
pub enum RenderError {
    /// A composition/layer construction error from the substrate compositor.
    Compose(mosaic_core::composite::Error),
    /// A layer declared [`CoverageMode::Explicit`] but the resolver supplied none.
    MissingExplicitCoverage,
    /// Explicit coverage length did not match the resolved grid.
    CoverageSizeMismatch { expected: usize, got: usize },
    /// The host resolver failed (unknown engine/facet/input, bad params, …).
    Resolve(String),
}

impl core::fmt::Display for RenderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RenderError::Compose(e) => write!(f, "composition error: {e}"),
            RenderError::MissingExplicitCoverage => {
                write!(
                    f,
                    "layer declared explicit coverage but the resolver supplied none"
                )
            }
            RenderError::CoverageSizeMismatch { expected, got } => {
                write!(f, "explicit coverage has {got} cells, expected {expected}")
            }
            RenderError::Resolve(msg) => write!(f, "layer resolve failed: {msg}"),
        }
    }
}

impl std::error::Error for RenderError {}

impl Composition {
    /// Serialize to pretty JSON — the shareable form.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parse from JSON.
    pub fn from_json(s: &str) -> Result<Composition, serde_json::Error> {
        serde_json::from_str(s)
    }
}

/// Render a [`Composition`] to text: resolve each layer through `resolver`, composite the
/// stack (painter's order) via the O4 primitive, and compose to validated text.
pub fn render(comp: &Composition, resolver: &mut dyn LayerResolver) -> Result<String, RenderError> {
    let mut canvas =
        Canvas::new(comp.canvas.cols, comp.canvas.rows).map_err(RenderError::Compose)?;
    for decl in &comp.layers {
        let ResolvedLayer {
            cols,
            rows,
            tokens,
            coverage,
        } = resolver.resolve(&decl.source)?;
        let n = (cols as usize).saturating_mul(rows as usize);
        let cov = match &decl.coverage {
            CoverageMode::Opaque => vec![1.0; n],
            CoverageMode::KeyedOn { token } => tokens
                .iter()
                .map(|&t| if t == *token { 0.0 } else { 1.0 })
                .collect(),
            CoverageMode::Explicit => {
                let c = coverage.ok_or(RenderError::MissingExplicitCoverage)?;
                if c.len() != n {
                    return Err(RenderError::CoverageSizeMismatch {
                        expected: n,
                        got: c.len(),
                    });
                }
                c
            }
        };
        let layer = Layer::with_coverage(cols, rows, tokens, cov).map_err(RenderError::Compose)?;
        canvas.place(&layer, decl.at.row, decl.at.col, decl.blend.into());
    }
    Ok(canvas.into_text(comp.background))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: u32 = b'A' as u32;
    const B: u32 = b'B' as u32;
    const SP: u32 = b' ' as u32;

    /// A resolver returning fixed grids keyed by facet name — isolates the declarative
    /// layer from any engine.
    struct MockResolver;
    impl LayerResolver for MockResolver {
        fn resolve(&mut self, source: &LayerSource) -> Result<ResolvedLayer, RenderError> {
            match source.facet.as_str() {
                // A 3x1 opaque row of 'A'.
                "fill_a" => Ok(ResolvedLayer {
                    cols: 3,
                    rows: 1,
                    tokens: vec![A, A, A],
                    coverage: None,
                }),
                // A 3x1 row 'B _ B' (middle keyed-out when KeyedOn space).
                "holes_b" => Ok(ResolvedLayer {
                    cols: 3,
                    rows: 1,
                    tokens: vec![B, SP, B],
                    coverage: None,
                }),
                other => Err(RenderError::Resolve(format!("unknown facet {other:?}"))),
            }
        }
    }

    fn layer(facet: &str, blend: BlendSpec, coverage: CoverageMode) -> LayerDecl {
        LayerDecl {
            source: LayerSource {
                engine: "mock".into(),
                facet: facet.into(),
                input: "in".into(),
                params: serde_json::Value::Null,
            },
            at: Placement::default(),
            blend,
            coverage,
        }
    }

    #[test]
    fn json_round_trips() {
        let comp = Composition {
            canvas: CanvasSpec { cols: 3, rows: 1 },
            background: SP,
            layers: vec![
                layer("fill_a", BlendSpec::Over, CoverageMode::Opaque),
                layer(
                    "holes_b",
                    BlendSpec::Over,
                    CoverageMode::KeyedOn { token: SP },
                ),
            ],
        };
        let json = comp.to_json().unwrap();
        let back = Composition::from_json(&json).unwrap();
        assert_eq!(
            comp, back,
            "a Composition must survive a JSON round-trip unchanged"
        );
    }

    #[test]
    fn declarative_render_matches_expected_and_is_deterministic() {
        // 'A A A' under 'B _ B' (keyed on space) → the hole reveals the 'A' beneath.
        let comp = Composition {
            canvas: CanvasSpec { cols: 3, rows: 1 },
            background: SP,
            layers: vec![
                layer("fill_a", BlendSpec::Over, CoverageMode::Opaque),
                layer(
                    "holes_b",
                    BlendSpec::Over,
                    CoverageMode::KeyedOn { token: SP },
                ),
            ],
        };
        let text = render(&comp, &mut MockResolver).unwrap();
        assert_eq!(text, "BAB");
        assert_eq!(render(&comp, &mut MockResolver).unwrap(), text);
    }

    #[test]
    fn declarative_equals_imperative() {
        // The same result the imperative O4 primitive produces from the same grids.
        let comp = Composition {
            canvas: CanvasSpec { cols: 3, rows: 1 },
            background: SP,
            layers: vec![
                layer("fill_a", BlendSpec::Over, CoverageMode::Opaque),
                layer(
                    "holes_b",
                    BlendSpec::Over,
                    CoverageMode::KeyedOn { token: SP },
                ),
            ],
        };
        let declarative = render(&comp, &mut MockResolver).unwrap();

        let mut c = Canvas::new(3, 1).unwrap();
        c.place(
            &Layer::opaque(3, 1, vec![A, A, A]).unwrap(),
            0,
            0,
            Blend::Over,
        );
        c.place(
            &Layer::keyed(3, 1, vec![B, SP, B], SP).unwrap(),
            0,
            0,
            Blend::Over,
        );
        let imperative = c.into_text(SP);

        assert_eq!(declarative, imperative);
    }

    #[test]
    fn explicit_coverage_is_required_and_sized() {
        // The mock never supplies coverage, so Explicit mode must error cleanly.
        let comp = Composition {
            canvas: CanvasSpec { cols: 3, rows: 1 },
            background: SP,
            layers: vec![layer("fill_a", BlendSpec::Over, CoverageMode::Explicit)],
        };
        assert_eq!(
            render(&comp, &mut MockResolver),
            Err(RenderError::MissingExplicitCoverage)
        );
    }

    #[test]
    fn unknown_facet_surfaces_a_resolve_error() {
        let comp = Composition {
            canvas: CanvasSpec { cols: 1, rows: 1 },
            background: SP,
            layers: vec![layer("nope", BlendSpec::Over, CoverageMode::Opaque)],
        };
        assert!(matches!(
            render(&comp, &mut MockResolver),
            Err(RenderError::Resolve(_))
        ));
    }

    #[test]
    fn blend_names_serialize_lowercase() {
        // The wire form is stable and human-authorable.
        let comp = Composition {
            canvas: CanvasSpec { cols: 1, rows: 1 },
            background: SP,
            layers: vec![layer("fill_a", BlendSpec::Stipple, CoverageMode::Opaque)],
        };
        let json = comp.to_json().unwrap();
        assert!(
            json.contains("\"stipple\""),
            "blend serializes lowercase: {json}"
        );
    }
}
