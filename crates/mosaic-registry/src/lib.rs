//! # mosaic-registry
//!
//! The Facet registry: where certified Facets are stored, listed, fetched, and moderated.
//!
//! Persistence is behind the [`Store`] trait so the domain logic (and the server's
//! endpoints and auth) are tested against the pure-Rust [`InMemoryStore`], while a durable
//! backend implements the same contract. A Facet enters the registry already **certified**
//! (the server runs `mosaic-certify` on publish), moves to **published** or **rejected** by
//! a moderator, and never transitions any other way — the state machine lives in the server.

#![forbid(unsafe_code)]

use std::fmt;

use mosaic_certify::{AbiKind, Certificate, ProgramCertificate};
use serde::{Deserialize, Serialize};

mod memory;
mod redb_store;

pub use memory::InMemoryStore;
pub use redb_store::RedbStore;

/// Moderation state of a Facet. A Facet is `Certified` the moment it is stored (publishing
/// runs the gate), then a moderator moves it to `Published` or `Rejected`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FacetState {
    /// Certified on submission, awaiting a moderator's decision. Not publicly listed.
    Certified,
    /// Approved by a moderator; publicly listable and renderable by id.
    Published,
    /// Rejected by a moderator.
    Rejected,
}

impl FacetState {
    /// The stable lowercase slug (matches the `#[serde]` representation), for storage.
    pub const fn as_str(self) -> &'static str {
        match self {
            FacetState::Certified => "certified",
            FacetState::Published => "published",
            FacetState::Rejected => "rejected",
        }
    }

    /// Parse a state from its slug, for a persistent backend reading its own rows.
    pub fn parse(s: &str) -> Option<FacetState> {
        match s {
            "certified" => Some(FacetState::Certified),
            "published" => Some(FacetState::Published),
            "rejected" => Some(FacetState::Rejected),
            _ => None,
        }
    }
}

/// What a stored Facet actually is. The registry admits two kinds, each carrying its own
/// conformance certificate: a self-contained wasm module, or a DSL bytecode program that
/// runs on the one shared interpreter Facet. The bytes themselves (module or bytecode) are
/// stored alongside and fetched with [`Store::get_bytes`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum FacetArtifact {
    /// A self-contained wasm Facet module (gather or propagation).
    Wasm {
        /// The module's map ABI, from its certificate.
        abi_kind: AbiKind,
        /// Content hash of the module bytes, from its certificate.
        wasm_sha256: String,
        /// The conformance certificate.
        certificate: Certificate,
    },
    /// A DSL bytecode program run on the shared interpreter, targeting `engine`'s feature
    /// vocabulary (which fixes its `stride`).
    Program {
        /// The feature engine the program is authored for (e.g. `ascii`, `spectral`).
        engine: String,
        /// The program's declared feature stride (must match `engine`'s).
        stride: u32,
        /// Content hash of the bytecode, from its certificate.
        program_sha256: String,
        /// The conformance certificate.
        certificate: ProgramCertificate,
    },
}

/// The bare discriminant of a [`FacetArtifact`], for listings and content-type dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Wasm,
    Program,
}

impl FacetArtifact {
    /// This artifact's bare kind.
    pub fn kind(&self) -> ArtifactKind {
        match self {
            FacetArtifact::Wasm { .. } => ArtifactKind::Wasm,
            FacetArtifact::Program { .. } => ArtifactKind::Program,
        }
    }
}

/// Everything needed to persist a newly-certified Facet.
#[derive(Debug, Clone)]
pub struct NewFacet {
    /// Opaque registry id (assigned by the server).
    pub id: String,
    /// Author-supplied display name.
    pub name: String,
    /// Principal id of the author.
    pub author: String,
    /// Initial moderation state (`Certified`).
    pub state: FacetState,
    /// Creation time, unix seconds (the server stamps it).
    pub created_at: i64,
    /// What this Facet is (wasm module or DSL program) and its certificate.
    pub artifact: FacetArtifact,
    /// The stored bytes — the wasm module, or the DSL bytecode.
    pub bytes: Vec<u8>,
}

/// A stored Facet's full metadata (everything but the bytes; fetch those with
/// [`Store::get_bytes`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FacetRecord {
    pub id: String,
    pub name: String,
    pub author: String,
    pub state: FacetState,
    pub created_at: i64,
    pub artifact: FacetArtifact,
}

/// A lightweight listing entry — no certificate, no bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FacetSummary {
    pub id: String,
    pub name: String,
    pub author: String,
    pub kind: ArtifactKind,
    pub state: FacetState,
    pub created_at: i64,
}

impl FacetRecord {
    /// Project to a listing summary.
    pub fn summary(&self) -> FacetSummary {
        FacetSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            author: self.author.clone(),
            kind: self.artifact.kind(),
            state: self.state,
            created_at: self.created_at,
        }
    }
}

/// A listing filter. Public callers list only [`FacetState::Published`]; a moderator may
/// filter by any state.
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    /// Restrict to one state, or `None` for all.
    pub state: Option<FacetState>,
}

/// A registry backend failure — always a value, never a panic.
#[derive(Debug)]
pub enum StoreError {
    /// The backend (lock, database, disk) failed.
    Backend(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Backend(m) => write!(f, "registry backend error: {m}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// The registry persistence contract. `Send + Sync` so it lives in shared server state and
/// is called from blocking workers. Implementations must be internally synchronized.
pub trait Store: Send + Sync {
    /// Persist a newly-certified Facet, returning its stored metadata.
    fn insert(&self, facet: NewFacet) -> Result<FacetRecord, StoreError>;

    /// Fetch a Facet's metadata by id, or `None` if absent.
    fn get(&self, id: &str) -> Result<Option<FacetRecord>, StoreError>;

    /// Fetch a Facet's stored bytes by id (the wasm module or DSL bytecode), or `None` if
    /// absent.
    fn get_bytes(&self, id: &str) -> Result<Option<Vec<u8>>, StoreError>;

    /// List Facets matching `filter`, newest first.
    fn list(&self, filter: &ListFilter) -> Result<Vec<FacetSummary>, StoreError>;

    /// Set a Facet's moderation state. Returns `false` if the id is unknown.
    fn set_state(&self, id: &str, state: FacetState) -> Result<bool, StoreError>;
}
