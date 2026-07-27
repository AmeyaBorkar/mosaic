//! An in-memory [`Store`]: a mutex-guarded map. The domain, server endpoints, and auth are
//! all tested against it, and it is a fine dev/default backend; a durable backend
//! implements the same trait for production.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::{FacetRecord, FacetState, FacetSummary, ListFilter, NewFacet, Store, StoreError};

struct Stored {
    record: FacetRecord,
    wasm: Vec<u8>,
}

/// A process-local registry store. Data does not survive a restart — use for tests and
/// development; use a durable backend in production.
#[derive(Default)]
pub struct InMemoryStore {
    inner: Mutex<HashMap<String, Stored>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        InMemoryStore::default()
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, Stored>>, StoreError> {
        self.inner
            .lock()
            .map_err(|_| StoreError::Backend("registry lock poisoned".to_string()))
    }
}

impl Store for InMemoryStore {
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
        let mut map = self.lock()?;
        map.insert(
            record.id.clone(),
            Stored {
                record: record.clone(),
                wasm: facet.wasm,
            },
        );
        Ok(record)
    }

    fn get(&self, id: &str) -> Result<Option<FacetRecord>, StoreError> {
        Ok(self.lock()?.get(id).map(|s| s.record.clone()))
    }

    fn get_wasm(&self, id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self.lock()?.get(id).map(|s| s.wasm.clone()))
    }

    fn list(&self, filter: &ListFilter) -> Result<Vec<FacetSummary>, StoreError> {
        let map = self.lock()?;
        let mut out: Vec<FacetSummary> = map
            .values()
            .filter(|s| filter.state.is_none_or(|want| s.record.state == want))
            .map(|s| s.record.summary())
            .collect();
        // Newest first; ties broken by id so the order is total and stable.
        out.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }

    fn set_state(&self, id: &str, state: FacetState) -> Result<bool, StoreError> {
        let mut map = self.lock()?;
        match map.get_mut(id) {
            Some(stored) => {
                stored.record.state = state;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use mosaic_certify::{AbiKind, Certificate, Profile};

    use super::*;

    fn cert() -> Certificate {
        Certificate {
            certify_version: 1,
            wasm_sha256: "ab".repeat(32),
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
            wasm_sha256: "ab".repeat(32),
            state,
            created_at,
            certificate: cert(),
            wasm: vec![1, 2, 3, 4],
        }
    }

    #[test]
    fn insert_then_get_round_trips() {
        let store = InMemoryStore::new();
        store
            .insert(new_facet("id1", "Ramp", FacetState::Certified, 100))
            .unwrap();
        let got = store.get("id1").unwrap().unwrap();
        assert_eq!(got.name, "Ramp");
        assert_eq!(got.state, FacetState::Certified);
        assert_eq!(store.get_wasm("id1").unwrap().unwrap(), vec![1, 2, 3, 4]);
        assert!(store.get("missing").unwrap().is_none());
    }

    #[test]
    fn list_filters_by_state_newest_first() {
        let store = InMemoryStore::new();
        store
            .insert(new_facet("a", "A", FacetState::Published, 100))
            .unwrap();
        store
            .insert(new_facet("b", "B", FacetState::Certified, 200))
            .unwrap();
        store
            .insert(new_facet("c", "C", FacetState::Published, 300))
            .unwrap();

        let published = store
            .list(&ListFilter {
                state: Some(FacetState::Published),
            })
            .unwrap();
        assert_eq!(
            published.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            vec!["c", "a"], // newest first, only published
        );

        let all = store.list(&ListFilter::default()).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].id, "c"); // 300 is newest
    }

    #[test]
    fn set_state_transitions_and_reports_absence() {
        let store = InMemoryStore::new();
        store
            .insert(new_facet("id1", "Ramp", FacetState::Certified, 100))
            .unwrap();
        assert!(store.set_state("id1", FacetState::Published).unwrap());
        assert_eq!(
            store.get("id1").unwrap().unwrap().state,
            FacetState::Published
        );
        assert!(!store.set_state("missing", FacetState::Rejected).unwrap());
    }
}
