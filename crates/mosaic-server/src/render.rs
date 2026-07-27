//! `POST /v1/render` — the authoritative render.
//!
//! Mirrors the browser bridge's three-step pipeline (`mosaic-wasm`), run natively:
//!
//! 1. decode the input and extract features with the *same* engine functions the browser
//!    compiles to wasm (`tessera_*::feature::extract*`);
//! 2. run the Facet through the proven sandbox (`run_map` / `run_map_2d`);
//! 3. compose the tokens with the *same* shared composer (`mosaic_core::compose`).
//!
//! Because every step is the same source as the preview, a server render is bit-identical
//! to what the browser shows — this endpoint is the "truth" render for sharing and export.
//!
//! The **engine** selects the feature vocabulary (and thus the stride): `ascii` (stride 3),
//! `ascii-structural` (stride 64), `spectral` (stride 1). The Facet's certified **ABI kind**
//! selects gather vs propagation. A non-conformant inline Facet is refused before it runs.

use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use mosaic_certify::{AbiKind, Rejection, RejectionCode, check_profile};
use mosaic_core::compose::compose_codepoints;
use mosaic_runtime::{Facet, Limits, Sandbox};
use serde::{Deserialize, Serialize};
use tessera_ascii::{Grid, ImageRef};
use tessera_spectral::{SignalRef, SpectroGrid};

use crate::AppState;
use crate::error::ApiError;

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
}

/// Where the Facet module comes from. v1 supports an inline base64 module; the registry
/// `id` source is added with the registry.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FacetSource {
    #[serde(default)]
    inline: Option<String>,
}

impl FacetSource {
    fn wasm(&self) -> Result<Vec<u8>, ApiError> {
        match &self.inline {
            Some(b64) => decode_b64(b64, "facet.inline"),
            None => Err(ApiError::bad_request(
                "facet.inline (base64 wasm) is required",
            )),
        }
    }
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
}

impl Default for AsciiParams {
    fn default() -> Self {
        AsciiParams {
            cols: 100,
            cell_aspect: 2.0,
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

/// Success body: the composed text and its grid dimensions.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RenderOk {
    cols: u32,
    rows: u32,
    text: String,
}

pub async fn render_handler(
    State(state): State<AppState>,
    Json(req): Json<RenderRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // Decode the (potentially large) base64 payloads off the wasm path, then run the whole
    // CPU-bound pipeline (extract → run → compose) on a blocking worker.
    let job = build_job(req)?;
    let sandbox = state.sandbox.clone();
    let result = tokio::task::spawn_blocking(move || run_job(&sandbox, job))
        .await
        .map_err(|e| ApiError::internal(format!("render worker failed: {e}")))?;
    result.map(Json)
}

/// Owned, decoded render work — everything the blocking pipeline needs, no borrows.
enum RenderJob {
    Ascii {
        wasm: Vec<u8>,
        rgba: Vec<u8>,
        width: u32,
        height: u32,
        cols: u32,
        cell_aspect: f32,
        /// L2 structural vocabulary rather than L0+L1.
        structural: bool,
    },
    Spectral {
        wasm: Vec<u8>,
        pcm: Vec<f32>,
        sample_rate: u32,
        bands: u32,
        win: u32,
        hop: u32,
        fmin: f32,
        fmax: f32,
    },
}

fn build_job(req: RenderRequest) -> Result<RenderJob, ApiError> {
    match req {
        RenderRequest::Ascii {
            facet,
            input,
            params,
        } => Ok(ascii_job(facet, input, params, false)?),
        RenderRequest::AsciiStructural {
            facet,
            input,
            params,
        } => Ok(ascii_job(facet, input, params, true)?),
        RenderRequest::Spectral {
            facet,
            input,
            params,
        } => Ok(RenderJob::Spectral {
            wasm: facet.wasm()?,
            pcm: decode_pcm(&input.pcm)?,
            sample_rate: input.sample_rate,
            bands: params.bands,
            win: params.win,
            hop: params.hop,
            fmin: params.fmin,
            fmax: params.fmax,
        }),
    }
}

fn ascii_job(
    facet: FacetSource,
    input: ImageInput,
    params: AsciiParams,
    structural: bool,
) -> Result<RenderJob, ApiError> {
    Ok(RenderJob::Ascii {
        wasm: facet.wasm()?,
        rgba: decode_b64(&input.rgba, "input.rgba")?,
        width: input.width,
        height: input.height,
        cols: params.cols,
        cell_aspect: params.cell_aspect,
        structural,
    })
}

fn run_job(sandbox: &Sandbox, job: RenderJob) -> Result<RenderOk, ApiError> {
    match job {
        RenderJob::Ascii {
            wasm,
            rgba,
            width,
            height,
            cols,
            cell_aspect,
            structural,
        } => {
            let (abi, facet) = admit(sandbox, &wasm)?;
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
            run_and_compose(
                sandbox, &facet, abi, buf.cols, buf.rows, buf.stride, buf.data,
            )
        }
        RenderJob::Spectral {
            wasm,
            pcm,
            sample_rate,
            bands,
            win,
            hop,
            fmin,
            fmax,
        } => {
            let (abi, facet) = admit(sandbox, &wasm)?;
            let signal = SignalRef::new(&pcm, sample_rate)
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
            let grid = SpectroGrid::new(bands, win, hop, fmin, fmax);
            let buf = tessera_spectral::feature::extract(&signal, &grid)
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
            run_and_compose(
                sandbox, &facet, abi, buf.cols, buf.rows, buf.stride, buf.data,
            )
        }
    }
}

/// Statically admit the inline Facet and compile it in the authoritative host. A
/// non-conformant module is a 422 `Rejected`; a module that passes the static gate but
/// fails full wasm validation is a 422 `compile_failed`.
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

fn run_and_compose(
    sandbox: &Sandbox,
    facet: &Facet,
    abi: AbiKind,
    cols: u32,
    rows: u32,
    stride: u32,
    data: Vec<f32>,
) -> Result<RenderOk, ApiError> {
    let ncells = (cols as usize) * (rows as usize);
    let tokens = match abi {
        AbiKind::Gather => {
            sandbox.run_map(facet, Limits::default(), &data, ncells, stride as usize)
        }
        AbiKind::Propagation => sandbox.run_map_2d(
            facet,
            Limits::default(),
            &data,
            cols as usize,
            rows as usize,
            stride as usize,
        ),
    }
    .map_err(|e| ApiError::render_failed(format!("Facet failed during render: {e:#}")))?;
    let text = compose_codepoints(cols, rows, &tokens);
    Ok(RenderOk { cols, rows, text })
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
