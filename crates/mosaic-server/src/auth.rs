//! Bearer-token authentication and role-based authorization.
//!
//! A request presents `Authorization: Bearer <token>`. Tokens map to a [`Principal`] — an
//! id plus a set of [`Role`]s (author, moderator). Tokens are configured out of band
//! (`MOSAIC_TOKENS`, a JSON file kept out of the repo) and are stored **hashed**: the config
//! holds `SHA-256(token) -> Principal`, so no plaintext token sits in memory and a lookup is
//! a hash-map probe on the digest (constant-time in the token value, and the digest is
//! preimage-resistant). This is the standard opaque-API-token pattern.
//!
//! Handlers extract [`AuthedPrincipal`] (401 if absent/invalid) or [`OptionalPrincipal`]
//! (anonymous if no header, 401 if a header is present but invalid) and check roles.

use std::collections::{BTreeSet, HashMap};

use anyhow::{Context, anyhow, bail};
use axum::extract::FromRequestParts;
use axum::http::header;
use axum::http::request::Parts;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::AppState;
use crate::error::ApiError;

/// A capability a principal may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// May publish Facets.
    Author,
    /// May moderate (publish/reject) submitted Facets.
    Moderator,
}

impl Role {
    /// The stable slug used in config and API output.
    pub const fn as_str(self) -> &'static str {
        match self {
            Role::Author => "author",
            Role::Moderator => "moderator",
        }
    }

    fn parse(s: &str) -> Option<Role> {
        match s {
            "author" => Some(Role::Author),
            "moderator" => Some(Role::Moderator),
            _ => None,
        }
    }
}

/// An authenticated caller: an id and the roles it holds.
#[derive(Debug, Clone)]
pub struct Principal {
    /// Opaque principal id (also recorded as a Facet's author).
    pub id: String,
    /// The roles this principal holds.
    pub roles: BTreeSet<Role>,
}

impl Principal {
    /// Whether this principal holds `role`.
    pub fn has_role(&self, role: Role) -> bool {
        self.roles.contains(&role)
    }

    /// Require `role`, else a 403.
    pub fn require(&self, role: Role) -> Result<(), ApiError> {
        if self.has_role(role) {
            Ok(())
        } else {
            Err(ApiError::forbidden(format!(
                "this action requires the '{}' role",
                role.as_str()
            )))
        }
    }

    /// The principal's roles as sorted slugs (for API output).
    pub fn role_slugs(&self) -> Vec<&'static str> {
        self.roles.iter().map(|r| r.as_str()).collect()
    }
}

/// One entry of the token config file: a secret token, the principal id it authenticates,
/// and the roles it grants.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenEntry {
    pub token: String,
    pub id: String,
    #[serde(default)]
    pub roles: Vec<String>,
}

/// The server's token table: `SHA-256(token) -> Principal`.
#[derive(Default)]
pub struct AuthConfig {
    by_token_hash: HashMap<[u8; 32], Principal>,
}

impl AuthConfig {
    /// An empty table — every request authenticates as anonymous / fails bearer auth. The
    /// default when no `MOSAIC_TOKENS` is configured.
    pub fn empty() -> Self {
        AuthConfig::default()
    }

    /// Build a table from config entries, rejecting an unknown role or a duplicate token.
    pub fn from_entries(entries: Vec<TokenEntry>) -> anyhow::Result<Self> {
        let mut by_token_hash = HashMap::new();
        for entry in entries {
            let mut roles = BTreeSet::new();
            for role in &entry.roles {
                roles.insert(Role::parse(role).ok_or_else(|| {
                    anyhow!("unknown role {role:?} for principal {:?}", entry.id)
                })?);
            }
            let principal = Principal {
                id: entry.id,
                roles,
            };
            if by_token_hash
                .insert(sha256(entry.token.as_bytes()), principal)
                .is_some()
            {
                bail!("duplicate token in auth config");
            }
        }
        Ok(AuthConfig { by_token_hash })
    }

    /// Load the token table from the file named by `MOSAIC_TOKENS`, or an empty table if the
    /// variable is unset. The file is a JSON array of [`TokenEntry`]; keep it out of the repo.
    pub fn from_env() -> anyhow::Result<Self> {
        match std::env::var("MOSAIC_TOKENS") {
            Ok(path) => {
                let text = std::fs::read_to_string(&path)
                    .with_context(|| format!("reading MOSAIC_TOKENS file {path:?}"))?;
                let entries: Vec<TokenEntry> =
                    serde_json::from_str(&text).context("parsing MOSAIC_TOKENS JSON")?;
                AuthConfig::from_entries(entries)
            }
            Err(_) => Ok(AuthConfig::empty()),
        }
    }

    /// Resolve a bearer token to its principal, or `None` if unknown.
    pub fn lookup(&self, token: &str) -> Option<&Principal> {
        self.by_token_hash.get(&sha256(token.as_bytes()))
    }

    /// Number of configured principals.
    pub fn len(&self) -> usize {
        self.by_token_hash.len()
    }

    /// Whether no tokens are configured.
    pub fn is_empty(&self) -> bool {
        self.by_token_hash.is_empty()
    }
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// Extract the bearer token from the `Authorization` header (scheme is case-insensitive).
fn bearer(parts: &Parts) -> Option<&str> {
    let value = parts.headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

/// A required authenticated principal. Rejects with 401 when the token is missing or invalid.
pub struct AuthedPrincipal(pub Principal);

impl FromRequestParts<AppState> for AuthedPrincipal {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer(parts).ok_or_else(|| ApiError::unauthorized("missing bearer token"))?;
        let principal = state
            .auth
            .lookup(token)
            .ok_or_else(|| ApiError::unauthorized("invalid token"))?;
        Ok(AuthedPrincipal(principal.clone()))
    }
}

/// An optional principal: `None` when no `Authorization` header is present (anonymous), but
/// still a 401 when a header is present yet malformed or invalid. Used where visibility
/// depends on who is asking.
pub struct OptionalPrincipal(pub Option<Principal>);

impl FromRequestParts<AppState> for OptionalPrincipal {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if parts.headers.get(header::AUTHORIZATION).is_none() {
            return Ok(OptionalPrincipal(None));
        }
        let token = bearer(parts)
            .ok_or_else(|| ApiError::unauthorized("malformed Authorization header"))?;
        let principal = state
            .auth
            .lookup(token)
            .ok_or_else(|| ApiError::unauthorized("invalid token"))?;
        Ok(OptionalPrincipal(Some(principal.clone())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> AuthConfig {
        AuthConfig::from_entries(vec![
            TokenEntry {
                token: "author-secret".to_string(),
                id: "alice".to_string(),
                roles: vec!["author".to_string()],
            },
            TokenEntry {
                token: "mod-secret".to_string(),
                id: "max".to_string(),
                roles: vec!["author".to_string(), "moderator".to_string()],
            },
        ])
        .unwrap()
    }

    #[test]
    fn lookup_resolves_tokens_and_roles() {
        let cfg = config();
        let alice = cfg.lookup("author-secret").unwrap();
        assert_eq!(alice.id, "alice");
        assert!(alice.has_role(Role::Author));
        assert!(!alice.has_role(Role::Moderator));

        let max = cfg.lookup("mod-secret").unwrap();
        assert!(max.has_role(Role::Moderator));

        assert!(cfg.lookup("wrong").is_none());
    }

    #[test]
    fn unknown_role_is_rejected() {
        let err = AuthConfig::from_entries(vec![TokenEntry {
            token: "t".to_string(),
            id: "x".to_string(),
            roles: vec!["wizard".to_string()],
        }]);
        assert!(err.is_err());
    }

    #[test]
    fn duplicate_token_is_rejected() {
        let err = AuthConfig::from_entries(vec![
            TokenEntry {
                token: "same".to_string(),
                id: "a".to_string(),
                roles: vec![],
            },
            TokenEntry {
                token: "same".to_string(),
                id: "b".to_string(),
                roles: vec![],
            },
        ]);
        assert!(err.is_err());
    }

    #[test]
    fn empty_config_authenticates_nobody() {
        assert!(AuthConfig::empty().lookup("anything").is_none());
    }
}
