//! The certificate and the execution layer: admit a Facet, then fingerprint its
//! observable behavior with golden `(features -> tokens)` probes from the proven native
//! host.
//!
//! [`certify`] is the authority. It runs the static [`check_profile`] gate, compiles the
//! module in the authoritative sandbox ([`mosaic_runtime`]), and runs a deterministic
//! probe suite through it. The resulting [`Certificate`] carries the tokens the native
//! host produced for each probe — the golden the browser must reproduce, so
//! `preview == render` (decision D9) is a checked property for *this* Facet, bound to its
//! exact bytes by a content hash.
//!
//! The probe suite is a **representative sample**, not a proof over all inputs: it spans a
//! spread of feature values across a few strides and grid shapes. Equivalence outside the
//! probed points is not claimed — the same honest scope as the shipped golden vectors.

use std::fmt::Write as _;

use mosaic_runtime::{Limits, Sandbox};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::profile::{AbiKind, Profile, Rejection, RejectionCode, check_profile};

/// Schema version of the certificate. Bump on any change to the probe suite or the
/// certificate shape, so a stale certificate is never mistaken for a current one.
pub const CERTIFY_VERSION: u32 = 1;

/// Base seed for the deterministic probe generator. No `rand`/clock is used, so the
/// golden is stable across machines and runs (a hard requirement for the committed fixture
/// and the browser cross-check). Same constant the DSL golden uses, for consistency.
/// Shared with the DSL-program probe suite ([`crate::program`]).
pub(crate) const PROBE_SEED: u64 = 0x0DDB_1A5E_5BAD_5EED;

/// The observable result of running one probe through the authoritative host. Recorded so
/// the browser must reproduce it: identical `tokens`, or *also* fail to produce tokens.
/// A trap carries no message — native and browser trap texts differ, so only the
/// implementation-independent fact "produced these tokens" / "did not produce tokens" is
/// the checkable contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ProbeOutcome {
    /// The Facet produced exactly these `u32` tokens (one per cell).
    Tokens {
        /// One output token per cell, in row-major order.
        tokens: Vec<u32>,
    },
    /// The Facet trapped or errored on this probe (e.g. an out-of-bounds access). The
    /// browser must also fail to produce tokens for this probe.
    Trapped,
}

/// One golden probe: a deterministic feature buffer of `cols * rows` cells (each `stride`
/// little-endian `f32`s) and the [`ProbeOutcome`] the authoritative host produced for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    /// A stable, human-readable label (e.g. `gather_stride3`).
    pub name: String,
    /// Feature slots per cell.
    pub stride: u32,
    /// Grid columns. For a gather Facet this is the cell count and `rows` is 1.
    pub cols: u32,
    /// Grid rows. For a gather Facet this is 1.
    pub rows: u32,
    /// The feature buffer, length `cols * rows * stride`.
    pub features: Vec<f32>,
    /// What the native host produced.
    pub outcome: ProbeOutcome,
}

/// A Facet's conformance certificate: the profile it fits and the golden probe results,
/// bound to the exact module bytes by [`wasm_sha256`](Self::wasm_sha256). The registry
/// stores it; the browser replays [`probes`](Self::probes) to check `preview == render`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Certificate {
    /// The certificate schema version ([`CERTIFY_VERSION`]).
    pub certify_version: u32,
    /// Lowercase hex SHA-256 of the exact module bytes this certificate attests. A
    /// certificate is valid only for bytes hashing to this value.
    pub wasm_sha256: String,
    /// The Facet's map ABI, detected during admission.
    pub abi_kind: AbiKind,
    /// The conformance envelope enforced, by value.
    pub profile: Profile,
    /// The golden probe results.
    pub probes: Vec<Probe>,
}

impl Certificate {
    /// Serialize to stable, pretty JSON — the form the committed golden fixture and the
    /// registry use. Field and array order are deterministic, so re-emitting identical
    /// input yields byte-identical output (the golden-diff gate depends on this).
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parse a certificate from JSON.
    pub fn from_json(s: &str) -> Result<Certificate, serde_json::Error> {
        serde_json::from_str(s)
    }
}

/// The outcome of [`certify`]: either an admitted Facet with its certificate, or a precise
/// refusal. The two are the API's 200-with-certificate and 422-with-rejection branches;
/// distinguishing them (rather than a bare `Result`) keeps "the Facet is bad" separate
/// from "our host failed" — the latter is the caller's `Sandbox` to build, not this call.
pub enum CertifyOutcome {
    /// The Facet was admitted; here is its certificate.
    Certified(Certificate),
    /// The Facet was refused; here is why.
    Rejected(Rejection),
}

/// Admit and certify an untrusted Facet against the shared `sandbox`.
///
/// Runs the static [`check_profile`] gate, compiles the module in the authoritative host,
/// and fingerprints it with the deterministic probe suite. Returns [`CertifyOutcome`]:
/// `Certified` with the golden certificate, or `Rejected` with the reason. Infallible given
/// a sandbox — every failure mode of an untrusted module (non-conformant, won't compile,
/// traps on everything) is a `Rejected`, never a panic.
///
/// The `sandbox` is borrowed so the server builds it once and reuses it; a Facet cannot
/// affect another because each execution gets a fresh, zero-capability store.
pub fn certify(sandbox: &Sandbox, bytes: &[u8]) -> CertifyOutcome {
    let abi = match check_profile(bytes) {
        Ok(abi) => abi,
        Err(rejection) => return CertifyOutcome::Rejected(rejection),
    };

    // The authoritative validation. check_profile already rejected the structural cases;
    // a failure here means the module is otherwise invalid wasm — the Facet's fault.
    let facet = match sandbox.compile(bytes) {
        Ok(facet) => facet,
        Err(e) => {
            return CertifyOutcome::Rejected(Rejection::new(
                RejectionCode::CompileFailed,
                format!("Facet failed to compile in the authoritative host: {e:#}"),
            ));
        }
    };

    let specs = probe_suite(abi);
    let mut probes = Vec::with_capacity(specs.len());
    let mut produced_tokens = false;
    for spec in specs {
        let ncells = (spec.cols as usize) * (spec.rows as usize);
        let result = match abi {
            AbiKind::Gather => sandbox.run_map(
                &facet,
                Limits::default(),
                &spec.features,
                ncells,
                spec.stride as usize,
            ),
            AbiKind::Propagation => sandbox.run_map_2d(
                &facet,
                Limits::default(),
                &spec.features,
                spec.cols as usize,
                spec.rows as usize,
                spec.stride as usize,
            ),
        };
        let outcome = match result {
            Ok(tokens) => {
                produced_tokens = true;
                ProbeOutcome::Tokens { tokens }
            }
            Err(_) => ProbeOutcome::Trapped,
        };
        probes.push(Probe {
            name: spec.name,
            stride: spec.stride,
            cols: spec.cols,
            rows: spec.rows,
            features: spec.features,
            outcome,
        });
    }

    if !produced_tokens {
        return CertifyOutcome::Rejected(Rejection::new(
            RejectionCode::NeverProducesTokens,
            "Facet produced no tokens on any probe (it trapped on every one)",
        ));
    }

    CertifyOutcome::Certified(Certificate {
        certify_version: CERTIFY_VERSION,
        wasm_sha256: sha256_hex(bytes),
        abi_kind: abi,
        profile: Profile::current(),
        probes,
    })
}

/// A probe before it is run: the deterministic input, without an outcome.
struct ProbeSpec {
    name: String,
    stride: u32,
    cols: u32,
    rows: u32,
    features: Vec<f32>,
}

fn probe_suite(abi: AbiKind) -> Vec<ProbeSpec> {
    match abi {
        AbiKind::Gather => gather_probes(),
        AbiKind::Propagation => propagation_probes(),
    }
}

/// Gather probes across a representative range of strides 1..=8, one row of cells each — a
/// gather Facet reads fixed slots, so proving it across these strides pins its stride-invariant
/// behaviour for the browser to replay. The range spans the per-cell gather engines whose parity
/// rests on these per-Facet probes: `spectral` (stride 1) and `ascii` (stride 8) — the smallest
/// and largest per-cell gather engine strides — plus every stride between. It deliberately does
/// *not* reach `ascii-structural` (stride 64): that engine's
/// browser≡native parity is proven end-to-end by the render golden's `structuralText` — one
/// `glyph-atlas` matcher compiled both ways — so a probe at 64 would be redundant here, not a
/// missing guarantee.
fn gather_probes() -> Vec<ProbeSpec> {
    let mut specs = Vec::new();
    for &stride in &[1u32, 2, 3, 4, 5, 6, 7, 8] {
        let ncells = 24u32;
        let features = probe_features(
            PROBE_SEED ^ (0x9E37_79B9 * u64::from(stride)),
            ncells,
            1,
            stride,
        );
        specs.push(ProbeSpec {
            name: format!("gather_stride{stride}"),
            stride,
            cols: ncells,
            rows: 1,
            features,
        });
    }
    specs
}

/// Propagation probes: a few small 2-D grids, so a feedback method (e.g. error diffusion)
/// has real neighbourhood structure to traverse.
fn propagation_probes() -> Vec<ProbeSpec> {
    let mut specs = Vec::new();
    for (i, &(cols, rows, stride)) in [(8u32, 8u32, 1u32), (6, 4, 1), (5, 3, 2)]
        .iter()
        .enumerate()
    {
        let features = probe_features(
            PROBE_SEED ^ (0xABCD_1234 * (i as u64 + 1)),
            cols,
            rows,
            stride,
        );
        specs.push(ProbeSpec {
            name: format!("grid_{cols}x{rows}_stride{stride}"),
            stride,
            cols,
            rows,
            features,
        });
    }
    specs
}

/// A deterministic feature buffer of `cols * rows * stride` values spanning `[-0.5, 1.5]`
/// (so below-0, in-range, and above-1 are all exercised), with the ramp endpoints 0.0 and
/// 1.0 pinned onto slot 0 of the first two cells so a density facet always sees them.
/// Shared with the DSL-program probe suite ([`crate::program`]).
pub(crate) fn probe_features(seed: u64, cols: u32, rows: u32, stride: u32) -> Vec<f32> {
    let n = (cols as usize) * (rows as usize) * (stride as usize);
    let mut rng = XorShift64(seed | 1);
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        v.push(rng.next_f32() * 2.0 - 0.5);
    }
    if n >= 1 {
        v[0] = 0.0; // cell 0, slot 0
    }
    let cell1_slot0 = stride as usize; // cell 1, slot 0
    if cell1_slot0 < n {
        v[cell1_slot0] = 1.0;
    }
    v
}

/// A tiny deterministic PRNG (xorshift64) for probe features. Not for cryptographic use —
/// its only job is a stable, well-spread feature spread.
pub(crate) struct XorShift64(pub(crate) u64);

impl XorShift64 {
    /// Next value in `[0, 1)`.
    pub(crate) fn next_f32(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Lowercase hex SHA-256 of `bytes`. Shared with the DSL-program certificate.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real committed Facet fixtures (built from source by scripts/build-facets.sh, so they
    // are the exact bytes CI gates). facet_ramp is a gather Facet; facet_dither exports
    // run2d (propagation).
    const FACET_RAMP: &[u8] = include_bytes!("../../tessera-ascii/tests/facet_ramp.wasm");
    const FACET_DITHER: &[u8] = include_bytes!("../../tessera-ascii/tests/facet_dither.wasm");

    fn certified(bytes: &[u8]) -> Certificate {
        let sandbox = Sandbox::new().unwrap();
        match certify(&sandbox, bytes) {
            CertifyOutcome::Certified(cert) => cert,
            CertifyOutcome::Rejected(r) => panic!("expected certification, got rejection: {r}"),
        }
    }

    #[test]
    fn certifies_gather_facet() {
        let cert = certified(FACET_RAMP);
        assert_eq!(cert.certify_version, CERTIFY_VERSION);
        assert_eq!(cert.abi_kind, AbiKind::Gather);
        assert_eq!(cert.wasm_sha256.len(), 64);
        assert_eq!(cert.probes.len(), 8);
        // A well-formed ramp Facet produces tokens on every in-bounds probe.
        for probe in &cert.probes {
            assert!(
                matches!(probe.outcome, ProbeOutcome::Tokens { .. }),
                "ramp Facet unexpectedly trapped on {}",
                probe.name
            );
        }
    }

    #[test]
    fn certifies_propagation_facet() {
        let cert = certified(FACET_DITHER);
        assert_eq!(cert.abi_kind, AbiKind::Propagation);
        assert_eq!(cert.probes.len(), 3);
    }

    #[test]
    fn certification_is_deterministic() {
        let a = certified(FACET_RAMP);
        let b = certified(FACET_RAMP);
        assert_eq!(
            a, b,
            "certifying identical bytes must produce an identical certificate"
        );
    }

    #[test]
    fn hash_binds_to_bytes() {
        let cert = certified(FACET_RAMP);
        assert_eq!(cert.wasm_sha256, sha256_hex(FACET_RAMP));
        assert_ne!(cert.wasm_sha256, sha256_hex(FACET_DITHER));
    }

    #[test]
    fn certificate_round_trips_json() {
        let cert = certified(FACET_RAMP);
        let json = cert.to_json_pretty().unwrap();
        let back = Certificate::from_json(&json).unwrap();
        assert_eq!(cert, back);
    }

    #[test]
    fn rejects_non_conformant_before_execution() {
        // An import fails the static gate; certify surfaces the same rejection.
        let wat = r#"
            (module
              (import "env" "evil" (func))
              (memory (export "memory") 1 16)
              (func (export "alloc") (param i32) (result i32) (i32.const 0))
              (func (export "run") (param i32 i32 i32 i32)))
        "#;
        let bytes = wat::parse_str(wat).unwrap();
        let sandbox = Sandbox::new().unwrap();
        match certify(&sandbox, &bytes) {
            CertifyOutcome::Rejected(r) => assert_eq!(r.code, RejectionCode::Import),
            CertifyOutcome::Certified(_) => panic!("an importing Facet must be rejected"),
        }
    }

    #[test]
    fn rejects_facet_that_only_traps() {
        // Passes the static gate (has memory/alloc/run) but `run` always traps, so no probe
        // yields tokens.
        let wat = r#"
            (module
              (memory (export "memory") 1 16)
              (func (export "alloc") (param i32) (result i32) (i32.const 0))
              (func (export "run") (param i32 i32 i32 i32) (unreachable)))
        "#;
        let bytes = wat::parse_str(wat).unwrap();
        let sandbox = Sandbox::new().unwrap();
        match certify(&sandbox, &bytes) {
            CertifyOutcome::Rejected(r) => assert_eq!(r.code, RejectionCode::NeverProducesTokens),
            CertifyOutcome::Certified(_) => {
                panic!("a Facet that traps on every probe must be rejected")
            }
        }
    }
}
