//! A durable [`Store`] backed by [`redb`] — a pure-Rust, embedded, ACID key-value store.
//! No C toolchain, no separate server process: the registry survives restarts with nothing
//! to provision. Each Facet is two rows keyed by id — its metadata (JSON) and its module
//! bytes — written in one transaction, so a Facet is never half-stored.

use std::path::Path;

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

use crate::{FacetRecord, FacetState, FacetSummary, ListFilter, NewFacet, Store, StoreError};

/// id → JSON-encoded [`FacetRecord`] (metadata + certificate).
const RECORDS: TableDefinition<&str, &[u8]> = TableDefinition::new("facet_records");
/// id → module bytes.
const WASM: TableDefinition<&str, &[u8]> = TableDefinition::new("facet_wasm");

/// A durable registry store. Cloneable handles are unnecessary — wrap in an `Arc` to share.
pub struct RedbStore {
    db: Database,
}

/// Map any backend error to a [`StoreError`].
fn backend(e: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(e.to_string())
}

impl RedbStore {
    /// Open the registry database at `path`, creating it (and its tables) if absent.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let db = Database::create(path).map_err(backend)?;
        // Materialize both tables on first open so a read before any write does not error.
        let txn = db.begin_write().map_err(backend)?;
        {
            txn.open_table(RECORDS).map_err(backend)?;
            txn.open_table(WASM).map_err(backend)?;
        }
        txn.commit().map_err(backend)?;
        Ok(RedbStore { db })
    }
}

impl Store for RedbStore {
    fn insert(&self, facet: NewFacet) -> Result<FacetRecord, StoreError> {
        let record = FacetRecord {
            id: facet.id,
            name: facet.name,
            author: facet.author,
            abi_kind: facet.abi_kind,
            wasm_sha256: facet.wasm_sha256,
            state: facet.state,
            created_at: facet.created_at,
            certificate: facet.certificate,
        };
        let json = serde_json::to_vec(&record).map_err(backend)?;

        let txn = self.db.begin_write().map_err(backend)?;
        {
            let mut records = txn.open_table(RECORDS).map_err(backend)?;
            records
                .insert(record.id.as_str(), json.as_slice())
                .map_err(backend)?;
            let mut wasm = txn.open_table(WASM).map_err(backend)?;
            wasm.insert(record.id.as_str(), facet.wasm.as_slice())
                .map_err(backend)?;
        }
        txn.commit().map_err(backend)?;
        Ok(record)
    }

    fn get(&self, id: &str) -> Result<Option<FacetRecord>, StoreError> {
        let txn = self.db.begin_read().map_err(backend)?;
        let records = txn.open_table(RECORDS).map_err(backend)?;
        match records.get(id).map_err(backend)? {
            Some(guard) => Ok(Some(
                serde_json::from_slice(guard.value()).map_err(backend)?,
            )),
            None => Ok(None),
        }
    }

    fn get_wasm(&self, id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let txn = self.db.begin_read().map_err(backend)?;
        let wasm = txn.open_table(WASM).map_err(backend)?;
        match wasm.get(id).map_err(backend)? {
            Some(guard) => Ok(Some(guard.value().to_vec())),
            None => Ok(None),
        }
    }

    fn list(&self, filter: &ListFilter) -> Result<Vec<FacetSummary>, StoreError> {
        let txn = self.db.begin_read().map_err(backend)?;
        let records = txn.open_table(RECORDS).map_err(backend)?;
        let mut out = Vec::new();
        for row in records.iter().map_err(backend)? {
            let (_id, value) = row.map_err(backend)?;
            let record: FacetRecord = serde_json::from_slice(value.value()).map_err(backend)?;
            if filter.state.is_none_or(|want| record.state == want) {
                out.push(record.summary());
            }
        }
        out.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }

    fn set_state(&self, id: &str, state: FacetState) -> Result<bool, StoreError> {
        let txn = self.db.begin_write().map_err(backend)?;
        let found = {
            let mut records = txn.open_table(RECORDS).map_err(backend)?;
            // Read + decode into an owned record so the read guard is released before the write.
            let current: Option<FacetRecord> = match records.get(id).map_err(backend)? {
                Some(guard) => Some(serde_json::from_slice(guard.value()).map_err(backend)?),
                None => None,
            };
            match current {
                Some(mut record) => {
                    record.state = state;
                    let json = serde_json::to_vec(&record).map_err(backend)?;
                    records.insert(id, json.as_slice()).map_err(backend)?;
                    true
                }
                None => false,
            }
        };
        txn.commit().map_err(backend)?;
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use mosaic_certify::{AbiKind, Certificate, Profile};
    use tempfile::TempDir;

    use super::*;

    fn cert() -> Certificate {
        Certificate {
            certify_version: 1,
            wasm_sha256: "cd".repeat(32),
            abi_kind: AbiKind::Gather,
            profile: Profile::current(),
            probes: vec![],
        }
    }

    fn new_facet(id: &str, name: &str, state: FacetState, created_at: i64) -> NewFacet {
        NewFacet {
            id: id.to_string(),
            name: name.to_string(),
            author: "author-1".to_string(),
            abi_kind: AbiKind::Gather,
            wasm_sha256: "cd".repeat(32),
            state,
            created_at,
            certificate: cert(),
            wasm: vec![9, 8, 7, 6],
        }
    }

    fn store(dir: &TempDir) -> RedbStore {
        RedbStore::open(dir.path().join("registry.redb")).unwrap()
    }

    #[test]
    fn insert_get_list_and_transition() {
        let dir = TempDir::new().unwrap();
        let s = store(&dir);
        s.insert(new_facet("a", "A", FacetState::Published, 100))
            .unwrap();
        s.insert(new_facet("b", "B", FacetState::Certified, 200))
            .unwrap();

        assert_eq!(s.get("a").unwrap().unwrap().name, "A");
        assert_eq!(s.get_wasm("b").unwrap().unwrap(), vec![9, 8, 7, 6]);
        assert!(s.get("missing").unwrap().is_none());

        let published = s
            .list(&ListFilter {
                state: Some(FacetState::Published),
            })
            .unwrap();
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].id, "a");

        assert!(s.set_state("b", FacetState::Published).unwrap());
        assert_eq!(s.get("b").unwrap().unwrap().state, FacetState::Published);
        assert!(!s.set_state("missing", FacetState::Rejected).unwrap());
    }

    #[test]
    fn data_survives_reopen() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("registry.redb");
        {
            let s = RedbStore::open(&path).unwrap();
            s.insert(new_facet("persist", "P", FacetState::Published, 42))
                .unwrap();
        } // dropped: the database file is closed
        let reopened = RedbStore::open(&path).unwrap();
        let got = reopened.get("persist").unwrap().unwrap();
        assert_eq!(got.name, "P");
        assert_eq!(got.created_at, 42);
    }
}
