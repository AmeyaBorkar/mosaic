//! `POST /v1/render` — the authoritative render.
//!
//! Mirrors the browser bridge's three-step pipeline (`mosaic-wasm`), run natively:
//!
//! 1. decode the input and extract features with the *same* engine functions the browser
//!    compiles to wasm (`tessera_*::feature::extract*`);
//! 2. run the Facet through the proven sandbox (`run_map` / `run_map_2d` for a wasm Facet,
//!    `run_program` on the shared interpreter for a DSL program);
//! 3. compose the tokens with the *same* shared composer (`mosaic_core::compose`).
//!
//! Because every step is the same source as the preview, a server render is bit-identical
//! to what the browser shows — this endpoint is the "truth" render for sharing and export.
//!
//! The **engine** selects the feature vocabulary (and thus the stride): `ascii` (stride 3),
//! `ascii-structural` (stride 64), `spectral` (stride 1). A wasm Facet's certified **ABI kind**
//! selects gather vs propagation; a DSL program is always gather. The Facet comes either
//! inline (a base64 wasm module, admitted before it runs) or by `id` from the registry (a
//! published wasm module or DSL program).

use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use mosaic_certify::{AbiKind, Rejection, RejectionCode, check_profile};
use mosaic_core::compose::compose_codepoints;
use mosaic_registry::{FacetArtifact, FacetState};
use mosaic_runtime::{Facet, Limits, Sandbox};
use serde::{Deserialize, Serialize};
use tessera_ascii::{Grid, ImageRef};
use tessera_spectral::{SignalRef, SpectroGrid};

use crate::AppState;
use crate::error::ApiError;

/// The feature stride of a render `engine`, or `None` if the name is not one this build
/// renders. Used at publish time to check a DSL program targets an engine whose stride it
/// declares; the render path itself compares against the extractor's actual stride.
pub(crate) fn engine_stride(engine: &str) -> Option<u32> {
    match engine {
        "ascii" => Some(3),
        "ascii-structural" => Some(64),
        "spectral" => Some(1),
        _ => None,
    }
}

/// Request body for `POST /v1/render`, discriminated by `engine`.
#[derive(Deserialize)]
#[serde(tag = "engine", rename_all = "kebab-case")]
pub enum RenderRequest {
    /// Image → ASCII, L0+L1 density/edge vocabulary (stride 3).
    Ascii {
        facet: FacetSource,
        input: ImageInput,
        #[serde(default)]
        params: AsciiParams,
    },
    /// Image → ASCII, L2 structural vocabulary (stride 64).
    AsciiStructural {
        facet: FacetSource,
        input: ImageInput,
        #[serde(default)]
        params: AsciiParams,
    },
    /// Audio PCM → spectrogram, band-energy vocabulary (stride 1).
    Spectral {
        facet: FacetSource,
        input: PcmInput,
        params: SpectralParams,
    },
    /// Image → coloured half-block pixel art (no Facet): each cell is `▀` with its top and
    /// bottom pixel-halves' mean colours.
    Halfblock {
        input: ImageInput,
        #[serde(default)]
        params: AsciiParams,
    },
}

/// Where the Facet comes from: an inline base64 wasm module, or a published registry `id`
/// (a wasm module or a DSL program). Exactly one must be given.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FacetSource {
    #[serde(default)]
    inline: Option<String>,
    #[serde(default)]
    id: Option<String>,
}

/// Raw RGBA8 image input — the authoritative form (no decode ambiguity): the exact bytes
/// the client also previewed.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageInput {
    /// Base64 row-major 8-bit RGBA, `width * height * 4` bytes.
    rgba: String,
    width: u32,
    height: u32,
}

/// Mono PCM audio input.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PcmInput {
    /// Base64 little-endian `f32` samples.
    pcm: String,
    sample_rate: u32,
}

/// Grid parameters for the ASCII engines. Absent fields fall back to the engine defaults.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AsciiParams {
    cols: u32,
    cell_aspect: f32,
    /// When true, also return a per-cell tint colour, so the client can colourise the glyph
    /// render (ignored by the `halfblock` engine, which is always coloured).
    color: bool,
}

impl Default for AsciiParams {
    fn default() -> Self {
        AsciiParams {
            cols: 100,
            cell_aspect: 2.0,
            color: false,
        }
    }
}

/// Grid parameters for the spectral engine — required (they depend on the audio).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpectralParams {
    bands: u32,
    win: u32,
    hop: u32,
    fmin: f32,
    fmax: f32,
}

/// Success body: the grid dimensions plus whichever outputs the engine produced — `text`
/// (glyph render), `colors` (per-cell tint, when requested), or `glyph`/`fg`/`bg` (half-block
/// pixel art). Absent fields are omitted.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RenderOk {
    cols: u32,
    rows: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    colors: Option<Vec<u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    glyph: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fg: Option<Vec<u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bg: Option<Vec<u32>>,
}

pub async fn render_handler(
    State(state): State<AppState>,
    Json(req): Json<RenderRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // Resolve the Facet (which may hit the registry) and decode the (potentially large) base64
    // payloads off the wasm path, then run the whole CPU-bound pipeline (extract → run →
    // compose) on a blocking worker.
    let job = build_job(&state, req).await?;
    let sandbox = state.sandbox.clone();
    let interp = state.interp.clone();
    let result = tokio::task::spawn_blocking(move || run_job(&sandbox, &interp, job))
        .await
        .map_err(|e| ApiError::internal(format!("render worker failed: {e}")))?;
    result.map(Json)
}

/// A resolved Facet ready to run: either a wasm module (admitted and compiled at run time) or
/// a DSL program (run on the shared interpreter, at its declared `stride`).
enum FacetJob {
    Wasm(Vec<u8>),
    Program { bytes: Vec<u8>, stride: u32 },
}

/// Owned, decoded render work — everything the blocking pipeline needs, no borrows.
enum RenderJob {
    Ascii {
        facet: FacetJob,
        rgba: Vec<u8>,
        width: u32,
        height: u32,
        cols: u32,
        cell_aspect: f32,
        /// L2 structural vocabulary rather than L0+L1.
        structural: bool,
        /// Also return a per-cell tint colour.
        color: bool,
    },
    Halfblock {
        rgba: Vec<u8>,
        width: u32,
        height: u32,
        cols: u32,
        cell_aspect: f32,
    },
    Spectral {
        facet: FacetJob,
        pcm: Vec<f32>,
        sample_rate: u32,
        bands: u32,
        win: u32,
        hop: u32,
        fmin: f32,
        fmax: f32,
    },
}

async fn build_job(state: &AppState, req: RenderRequest) -> Result<RenderJob, ApiError> {
    match req {
        RenderRequest::Ascii {
            facet,
            input,
            params,
        } => {
            let facet = resolve_facet(state, facet).await?;
            ascii_job(facet, input, params, false)
        }
        RenderRequest::AsciiStructural {
            facet,
            input,
            params,
        } => {
            let facet = resolve_facet(state, facet).await?;
            ascii_job(facet, input, params, true)
        }
        RenderRequest::Spectral {
            facet,
            input,
            params,
        } => {
            let facet = resolve_facet(state, facet).await?;
            Ok(RenderJob::Spectral {
                facet,
                pcm: decode_pcm(&input.pcm)?,
                sample_rate: input.sample_rate,
                bands: params.bands,
                win: params.win,
                hop: params.hop,
                fmin: params.fmin,
                fmax: params.fmax,
            })
        }
        RenderRequest::Halfblock { input, params } => Ok(RenderJob::Halfblock {
            rgba: decode_b64(&input.rgba, "input.rgba")?,
            width: input.width,
            height: input.height,
            cols: params.cols,
            cell_aspect: params.cell_aspect,
        }),
    }
}

/// Resolve a [`FacetSource`] to a runnable [`FacetJob`]. An inline module is decoded here; an
/// `id` is looked up in the registry (a blocking read). Render is public, so only a
/// **published** Facet renders by id — an unpublished or unknown id is a 404 (its existence is
/// not revealed).
async fn resolve_facet(state: &AppState, source: FacetSource) -> Result<FacetJob, ApiError> {
    match (source.inline, source.id) {
        (Some(_), Some(_)) => Err(ApiError::bad_request(
            "provide `facet.inline` or `facet.id`, not both",
        )),
        (Some(b64), None) => Ok(FacetJob::Wasm(decode_b64(&b64, "facet.inline")?)),
        (None, Some(id)) => resolve_registry_facet(state, id).await,
        (None, None) => Err(ApiError::bad_request(
            "facet.inline (base64 wasm) or facet.id (a published facet) is required",
        )),
    }
}

async fn resolve_registry_facet(state: &AppState, id: String) -> Result<FacetJob, ApiError> {
    let registry = state.registry.clone();
    let lookup_id = id.clone();
    let record = tokio::task::spawn_blocking(move || registry.get(&lookup_id))
        .await
        .map_err(|e| ApiError::internal(format!("registry worker failed: {e}")))?
        .map_err(|e| ApiError::internal(format!("registry backend error: {e}")))?
        .ok_or_else(|| ApiError::not_found("no such facet"))?;
    if record.state != FacetState::Published {
        // Not public: do not reveal that a non-published Facet exists.
        return Err(ApiError::not_found("no such facet"));
    }

    let registry = state.registry.clone();
    let bytes = tokio::task::spawn_blocking(move || registry.get_bytes(&id))
        .await
        .map_err(|e| ApiError::internal(format!("registry worker failed: {e}")))?
        .map_err(|e| ApiError::internal(format!("registry backend error: {e}")))?
        .ok_or_else(|| ApiError::not_found("no such facet"))?;

    Ok(match record.artifact {
        FacetArtifact::Wasm { .. } => FacetJob::Wasm(bytes),
        FacetArtifact::Program { stride, .. } => FacetJob::Program { bytes, stride },
    })
}

fn ascii_job(
    facet: FacetJob,
    input: ImageInput,
    params: AsciiParams,
    structural: bool,
) -> Result<RenderJob, ApiError> {
    Ok(RenderJob::Ascii {
        facet,
        rgba: decode_b64(&input.rgba, "input.rgba")?,
        width: input.width,
        height: input.height,
        cols: params.cols,
        cell_aspect: params.cell_aspect,
        structural,
        color: params.color,
    })
}

fn run_job(sandbox: &Sandbox, interp: &Facet, job: RenderJob) -> Result<RenderOk, ApiError> {
    match job {
        RenderJob::Ascii {
            facet,
            rgba,
            width,
            height,
            cols,
            cell_aspect,
            structural,
            color,
        } => {
            if cols == 0 {
                return Err(ApiError::bad_request(
                    "params.cols must be greater than zero",
                ));
            }
            let image = ImageRef::new(width, height, &rgba)
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
            let grid = Grid::new(width, height, cols, cell_aspect);
            let ncells = (grid.cols() as usize)
                .checked_mul(grid.rows() as usize)
                .ok_or_else(|| ApiError::bad_request("grid dimensions overflow"))?;
            if ncells > tessera_ascii::MAX_CELLS {
                return Err(ApiError::bad_request(format!(
                    "grid has {ncells} cells, exceeding the maximum of {}",
                    tessera_ascii::MAX_CELLS
                )));
            }
            let buf = if structural {
                tessera_ascii::feature::extract_structural(&image, &grid)
            } else {
                tessera_ascii::feature::extract(&image, &grid)
            }
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
            let colors = if color {
                Some(
                    tessera_ascii::color::extract_cell_colors(&image, &grid)
                        .map_err(|e| ApiError::bad_request(e.to_string()))?,
                )
            } else {
                None
            };
            run_and_compose(
                sandbox, interp, &facet, buf.cols, buf.rows, buf.stride, buf.data, colors,
            )
        }
        RenderJob::Halfblock {
            rgba,
            width,
            height,
            cols,
            cell_aspect,
        } => {
            if cols == 0 {
                return Err(ApiError::bad_request(
                    "params.cols must be greater than zero",
                ));
            }
            let image = ImageRef::new(width, height, &rgba)
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
            let grid = Grid::new(width, height, cols, cell_aspect);
            let hb = tessera_ascii::color::render_halfblock(&image, &grid)
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
            Ok(RenderOk {
                cols: hb.cols,
                rows: hb.rows,
                text: None,
                colors: None,
                glyph: Some(tessera_ascii::color::HALF_BLOCK),
                fg: Some(hb.fg),
                bg: Some(hb.bg),
            })
        }
        RenderJob::Spectral {
            facet,
            pcm,
            sample_rate,
            bands,
            win,
            hop,
            fmin,
            fmax,
        } => {
            let signal = SignalRef::new(&pcm, sample_rate)
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
            let grid = SpectroGrid::new(bands, win, hop, fmin, fmax);
            let buf = tessera_spectral::feature::extract(&signal, &grid)
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
            run_and_compose(
                sandbox, interp, &facet, buf.cols, buf.rows, buf.stride, buf.data, None,
            )
        }
    }
}

/// Statically admit an inline wasm Facet and compile it in the authoritative host. A
/// non-conformant module is a 422 `Rejected`; a module that passes the static gate but fails
/// full wasm validation is a 422 `compile_failed`.
fn admit(sandbox: &Sandbox, wasm: &[u8]) -> Result<(AbiKind, Facet), ApiError> {
    let abi = check_profile(wasm).map_err(ApiError::Rejected)?;
    let facet = sandbox.compile(wasm).map_err(|e| {
        ApiError::Rejected(Rejection {
            code: RejectionCode::CompileFailed,
            message: format!("Facet failed to compile in the authoritative host: {e:#}"),
        })
    })?;
    Ok((abi, facet))
}

/// Run the resolved Facet over the extracted feature buffer and compose the tokens. A wasm
/// Facet is admitted and run through its certified ABI; a DSL program runs on the shared
/// interpreter at its declared stride (a stride that disagrees with the engine's is a 422).
#[allow(clippy::too_many_arguments)]
fn run_and_compose(
    sandbox: &Sandbox,
    interp: &Facet,
    facet: &FacetJob,
    cols: u32,
    rows: u32,
    stride: u32,
    data: Vec<f32>,
    colors: Option<Vec<u32>>,
) -> Result<RenderOk, ApiError> {
    let ncells = (cols as usize) * (rows as usize);
    let tokens = match facet {
        FacetJob::Wasm(wasm) => {
            let (abi, compiled) = admit(sandbox, wasm)?;
            match abi {
                AbiKind::Gather => {
                    sandbox.run_map(&compiled, Limits::default(), &data, ncells, stride as usize)
                }
                AbiKind::Propagation => sandbox.run_map_2d(
                    &compiled,
                    Limits::default(),
                    &data,
                    cols as usize,
                    rows as usize,
                    stride as usize,
                ),
            }
            .map_err(|e| ApiError::render_failed(format!("Facet failed during render: {e:#}")))?
        }
        FacetJob::Program {
            bytes,
            stride: program_stride,
        } => {
            if *program_stride != stride {
                return Err(ApiError::Rejected(Rejection {
                    code: RejectionCode::ProgramStrideMismatch,
                    message: format!(
                        "program declares stride {program_stride} but this render has stride {stride}"
                    ),
                }));
            }
            sandbox
                .run_program(
                    interp,
                    Limits::default(),
                    bytes,
                    &data,
                    ncells,
                    stride as usize,
                )
                .map_err(|e| {
                    ApiError::render_failed(format!("program failed during render: {e:#}"))
                })?
        }
    };
    let text = compose_codepoints(cols, rows, &tokens);
    Ok(RenderOk {
        cols,
        rows,
        text: Some(text),
        colors,
        glyph: None,
        fg: None,
        bg: None,
    })
}

/// Decode a base64 field, mapping failure to a 400 that names the field.
fn decode_b64(b64: &str, field: &str) -> Result<Vec<u8>, ApiError> {
    STANDARD
        .decode(b64.as_bytes())
        .map_err(|e| ApiError::bad_request(format!("`{field}` is not valid base64: {e}")))
}

/// Decode base64 little-endian `f32` PCM.
fn decode_pcm(b64: &str) -> Result<Vec<f32>, ApiError> {
    let bytes = decode_b64(b64, "input.pcm")?;
    if !bytes.len().is_multiple_of(4) {
        return Err(ApiError::bad_request(
            "input.pcm byte length must be a multiple of 4 (little-endian f32)",
        ));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}
