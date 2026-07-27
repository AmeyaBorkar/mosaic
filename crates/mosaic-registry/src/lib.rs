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

use mosaic_certify::{AbiKind, Certificate};
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

/// Everything needed to persist a newly-certified Facet.
#[derive(Debug, Clone)]
pub struct NewFacet {
    /// Opaque registry id (assigned by the server).
    pub id: String,
    /// Author-supplied display name.
    pub name: String,
    /// Principal id of the author.
    pub author: String,
    /// The Facet's ABI kind (from its certificate).
    pub abi_kind: AbiKind,
    /// Content hash of the module bytes (from its certificate).
    pub wasm_sha256: String,
    /// Initial moderation state (`Certified`).
    pub state: FacetState,
    /// Creation time, unix seconds (the server stamps it).
    pub created_at: i64,
    /// The conformance certificate.
    pub certificate: Certificate,
    /// The module bytes.
    pub wasm: Vec<u8>,
}

/// A stored Facet's full metadata (everything but the module bytes; fetch those with
/// [`Store::get_wasm`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FacetRecord {
    pub id: String,
    pub name: String,
    pub author: String,
    pub abi_kind: AbiKind,
    pub wasm_sha256: String,
    pub state: FacetState,
    pub created_at: i64,
    pub certificate: Certificate,
}

/// A lightweight listing entry — no certificate, no bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FacetSummary {
    pub id: String,
    pub name: String,
    pub author: String,
    pub abi_kind: AbiKind,
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
            abi_kind: self.abi_kind,
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

    /// Fetch a Facet's module bytes by id, or `None` if absent.
    fn get_wasm(&self, id: &str) -> Result<Option<Vec<u8>>, StoreError>;

    /// List Facets matching `filter`, newest first.
    fn list(&self, filter: &ListFilter) -> Result<Vec<FacetSummary>, StoreError>;

    /// Set a Facet's moderation state. Returns `false` if the id is unknown.
    fn set_state(&self, id: &str, state: FacetState) -> Result<bool, StoreError>;
}
