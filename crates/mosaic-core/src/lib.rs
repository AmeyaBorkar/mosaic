//! # mosaic-core
//!
//! The domain-agnostic core of **Mosaic** — the substrate layer from the project
//! vision. This crate defines the shapes every domain shares, so that a **Facet**
//! (a community-authored style) written against one **Tessera** (a domain engine)
//! is structurally comparable to a Facet in any other.
//!
//! The three layers (see `docs/architecture.md`):
//! - **Mosaic** — the platform substrate: registry, runtime, generated controls.
//! - **Tessera** — one engine per domain (ASCII, ANSI, halftone, data→art).
//! - **Facet** — the community style layer, many per Tessera.
//!
//! ## The engine contract (five slots)
//!
//! Every Tessera fills the same five slots. The modules below name them.
//!
//! | Slot | Question | Module |
//! |------|----------|--------|
//! | Input | What media does this engine take? | [`input`] |
//! | Unit | How is the media decomposed into pieces? | [`unit`] |
//! | Feature vocabulary | What may a unit measure about itself? | [`feature`] |
//! | Output primitive | What does one unit become? | [`output`] |
//! | Composition | How are the pieces reassembled? | [`compose`] |
//!
//! Two load-bearing decisions are now settled (see `docs/architecture.md`):
//! - **D5 / O1 — access model:** a unit is a pure function of features gathered
//!   over a bounded read-only neighborhood (radius R; R=0 = self-only), keeping
//!   the common path parallel and deterministic. Sequential feedback (e.g.
//!   error-diffusion dithering) is a separate, opt-in [`propagate`] capability.
//! - **D6 / O2 — first vocabulary (ASCII):** luminance + gradient + sub-cell
//!   structure. The concrete features live in the ASCII engine; here we define the
//!   [`feature`] *schema* shape that carries them across the Facet ABI.
//!
//! The [`manifest`] declares a Facet's user-facing parameters — the surface Mosaic
//! turns into controls automatically.

#![forbid(unsafe_code)]

/// Slot 1 — the media an engine accepts.
///
/// What kind of media a Tessera takes (an image, a dataset, …). The concrete
/// type is the Tessera's; kept abstract here pending a second domain (O5).
pub mod input {}

/// Slot 2 — how media is decomposed into workable units.
///
/// How a Tessera decomposes its input into units (a grid cell, a row, …), and
/// how units are addressed. The addressing space is domain-shaped (a 2-D grid
/// for ASCII, a 1-D sequence for data→art), so it stays abstract here (O5).
///
/// Access model (D5 / O1): a unit is evaluated as a pure function of its
/// [`crate::feature`]s, which may be gathered over a bounded neighborhood. The
/// Facet only ever sees the resulting features and returns one output, so
/// per-unit evaluation stays parallel and deterministic regardless of gather.
pub mod unit {}

/// Slot 3 — the feature vocabulary: what a unit may measure about itself.
///
/// A Tessera declares a [`FeatureSchema`]: the ordered, typed set of measurements
/// a unit exposes to a Facet. The runtime uses the schema to marshal each unit's
/// features into the flat `f32` buffer the (WASM) Facet reads — so the runtime is
/// generic while each Tessera defines its own concrete vocabulary.
///
/// For the first ASCII Tessera (D6 / O2) the fields are, illustratively:
/// `luminance` (L0, [`FeatureType::Scalar`]), `gradient` (L1,
/// [`FeatureType::Vector`] of magnitude+orientation, gathered), and `patch` (L2,
/// [`FeatureType::Patch`] sub-cell structure for glyph shape-matching).
pub mod feature {
    /// A Tessera's declared feature vocabulary: an ordered list of typed fields.
    ///
    /// Order is the ABI: fields lay out contiguously in the Facet's input buffer.
    /// Extend by appending, never by reordering, to keep existing Facets valid.
    #[derive(Debug, Clone, PartialEq)]
    pub struct FeatureSchema {
        pub fields: Vec<FeatureField>,
    }

    impl FeatureSchema {
        /// Total `f32` slots one unit's features occupy — the per-unit stride of
        /// the Facet input buffer.
        pub fn total_slots(&self) -> u32 {
            self.fields.iter().map(|f| f.ty.slots()).sum()
        }

        /// The widest neighbourhood any field reads (in units). The runtime pads
        /// the feature field by this radius so every unit can gather in-bounds.
        pub fn max_radius(&self) -> u16 {
            self.fields
                .iter()
                .map(|f| f.gather.radius())
                .max()
                .unwrap_or(0)
        }
    }

    /// One field in the vocabulary: an identifier, a wire type, and how far it
    /// reads around its unit.
    #[derive(Debug, Clone, PartialEq)]
    pub struct FeatureField {
        /// Identifier the Facet reads (e.g. `"luminance"`, `"gradient"`).
        pub key: String,
        pub ty: FeatureType,
        pub gather: Gather,
    }

    /// The wire type of a feature field — a small, deterministic numeric set that
    /// marshals cleanly and bit-reproducibly across the WASM boundary.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum FeatureType {
        /// A single `f32` (e.g. luminance, gradient magnitude).
        Scalar,
        /// A fixed-length vector of `f32` (e.g. mean RGB = 3, gradient = 2).
        Vector { len: u16 },
        /// A fixed 2-D patch of `f32` (e.g. an N×M sub-cell luminance patch, L2).
        Patch { rows: u16, cols: u16 },
    }

    impl FeatureType {
        /// Number of contiguous `f32` slots this field occupies.
        pub const fn slots(self) -> u32 {
            match self {
                FeatureType::Scalar => 1,
                FeatureType::Vector { len } => len as u32,
                FeatureType::Patch { rows, cols } => rows as u32 * cols as u32,
            }
        }
    }

    /// How far a feature reads around its unit (D5 / O1). `SelfOnly` is radius 0.
    /// Both variants are pure over the immutable input, so gather never breaks
    /// parallelism or determinism.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Gather {
        SelfOnly,
        /// Bounded neighbourhood; `radius` is measured in units.
        Neighborhood {
            radius: u16,
        },
    }

    impl Gather {
        /// Radius in units (0 for `SelfOnly`).
        pub const fn radius(self) -> u16 {
            match self {
                Gather::SelfOnly => 0,
                Gather::Neighborhood { radius } => radius,
            }
        }
    }
}

/// Slot 4 — the output primitive a single unit becomes.
///
/// What one unit maps to (a character, a colored glyph, a dot, a mark). Like
/// features, the concrete primitive is the Tessera's; the runtime marshals a
/// Facet's returned output per a declared output schema mirroring
/// [`crate::feature::FeatureSchema`].
pub mod output {}

/// Slot 5 — composition: how output primitives are reassembled into a whole.
///
/// How per-unit outputs recombine (a text grid, a raster, a plot). Owned by
/// the Tessera, which receives the grid of Facet outputs and produces the
/// final artifact.
pub mod compose {}

/// Opt-in sequential propagation (D5 / O1) — for the feedback class.
///
/// Most Facets are pure per-unit gather (parallel, deterministic). The one
/// genuinely-sequential pattern — feedback, e.g. error-diffusion dithering —
/// is confined here as an *opt-in* capability so it never taints the common
/// path.
///
/// A participating Facet does not scatter arbitrarily (that would break purity
/// and determinism). Instead it returns a *residual* alongside its output, and
/// the engine diffuses that residual to not-yet-processed units along a
/// declared kernel and traversal order. The Facet stays pure; the engine owns
/// the ordering, so the whole pass stays deterministic and reproducible.
///
/// The concrete capability type is designed alongside the first Facet that
/// needs it; the contract reserves the mechanism from day one.
pub mod propagate {}

/// A Facet's declared, user-facing parameters — the surface Mosaic renders into
/// controls automatically. The simple, config-level side of a style.
pub mod manifest {
    /// A Facet's full declared parameter surface. Mosaic generates one control per
    /// [`Param`] and passes the user's values to the Facet logic at render time.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Manifest {
        pub params: Vec<Param>,
    }

    /// One declared parameter: a stable key, a human label, optional help, and a
    /// typed control spec that drives the generated UI.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Param {
        /// Stable identifier the Facet logic reads (e.g. `"contrast"`).
        pub key: String,
        /// Human-facing label for the control (e.g. `"Contrast"`).
        pub label: String,
        /// Optional longer help/description text.
        pub help: Option<String>,
        /// The control's type, bounds, and default — the whole widget spec.
        pub control: Control,
    }

    /// The kind of control, its bounds, and its default value. Each variant maps
    /// to a specific auto-generated UI widget. Kept to broadly-universal control
    /// types; domain-specific controls (e.g. a glyph ramp) are added by Tesserae.
    #[derive(Debug, Clone, PartialEq)]
    pub enum Control {
        /// Continuous slider.
        Float {
            default: f64,
            min: f64,
            max: f64,
            step: Option<f64>,
        },
        /// Integer stepper / slider.
        Int { default: i64, min: i64, max: i64 },
        /// Checkbox.
        Bool { default: bool },
        /// Single choice from a fixed set (dropdown / segmented control).
        Choice {
            default: String,
            options: Vec<ChoiceOption>,
        },
        /// Free text, length-bounded.
        Text { default: String, max_len: u32 },
        /// Color picker.
        Color { default: Rgba },
    }

    /// One option in a [`Control::Choice`].
    #[derive(Debug, Clone, PartialEq)]
    pub struct ChoiceOption {
        pub value: String,
        pub label: String,
    }

    /// An 8-bit-per-channel RGBA color.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Rgba {
        pub r: u8,
        pub g: u8,
        pub b: u8,
        pub a: u8,
    }
}

#[cfg(test)]
mod tests {
    use crate::feature::{FeatureField, FeatureSchema, FeatureType, Gather};

    /// The buffer stride is the sum of every field's slots — this is the layout
    /// the Facet ABI relies on. Exercised with the ASCII vocabulary shape
    /// (D6 / O2): luminance (L0), gradient (L1, gathered), sub-cell patch (L2).
    #[test]
    fn feature_buffer_stride_sums_field_slots() {
        let schema = FeatureSchema {
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
                FeatureField {
                    key: "patch".into(),
                    ty: FeatureType::Patch { rows: 4, cols: 4 },
                    gather: Gather::SelfOnly,
                },
            ],
        };
        assert_eq!(schema.total_slots(), 1 + 2 + 16);
        assert_eq!(schema.max_radius(), 1);
    }

    #[test]
    fn schema_invariants_hold_over_random_fields() {
        // Deterministic xorshift PRNG (reproducible sweep, no dependency).
        let mut state: u64 = 0xDEAD_BEEF_CAFE_F00D;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 40) as u16
        };
        for _ in 0..500 {
            let n = (next() % 8) as usize;
            let mut fields = Vec::new();
            let mut expect_slots = 0u32;
            let mut expect_radius = 0u16;
            for i in 0..n {
                let ty = match next() % 3 {
                    0 => FeatureType::Scalar,
                    1 => FeatureType::Vector {
                        len: 1 + next() % 8,
                    },
                    _ => FeatureType::Patch {
                        rows: 1 + next() % 8,
                        cols: 1 + next() % 8,
                    },
                };
                let radius = next() % 4;
                let gather = if radius == 0 {
                    Gather::SelfOnly
                } else {
                    Gather::Neighborhood { radius }
                };
                expect_slots += ty.slots();
                expect_radius = expect_radius.max(radius);
                fields.push(FeatureField {
                    key: format!("f{i}"),
                    ty,
                    gather,
                });
            }
            let schema = FeatureSchema { fields };
            // total_slots is exactly the sum of field slots; max_radius the max gather.
            assert_eq!(schema.total_slots(), expect_slots);
            assert_eq!(schema.max_radius(), expect_radius);
        }
    }
}
