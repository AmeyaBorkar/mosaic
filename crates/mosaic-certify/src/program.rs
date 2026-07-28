//! Certifying a **DSL program** — the other kind of Facet the registry admits.
//!
//! A wasm Facet ([`certify`](crate::certify)) is a self-contained module. A *program* Facet
//! is a compact `mosaic-vm` bytecode blob that runs on the **shared interpreter Facet**
//! (`facet-interp`, shipped with this crate as [`INTERP_WASM`]): the author writes the DSL,
//! it compiles to bytecode, and everyone runs it on the one trusted interpreter. Only the
//! bytecode is untrusted — the interpreter re-validates it before every run.
//!
//! [`certify_program`] is the authority, mirroring [`certify`](crate::certify::certify) step
//! for step but for bytecode:
//!
//! 1. **Validate natively** with [`mosaic_vm::validate`] — this both admits the program (a
//!    precise, machine-readable [`Rejection`] on failure, unlike the interpreter's bare
//!    `-1`) and reads its declared feature **stride**.
//! 2. **Probe through the interpreter** in the authoritative sandbox
//!    ([`Sandbox::run_program`]) — the exact artifact the browser also runs — recording the
//!    golden `(features -> tokens)` the browser must reproduce. The native `mosaic_vm` and
//!    the sandboxed interpreter are byte-identical by construction (see `mosaic-runtime`'s
//!    `dsl` test), so this is the same `preview == render` guarantee, now for authored DSL.
//!
//! The golden is bound to the exact program bytes by a SHA-256, exactly as the wasm
//! certificate binds to its module bytes.

use mosaic_runtime::{Facet, Limits, Sandbox};
use mosaic_vm::VmError;
use serde::{Deserialize, Serialize};

use crate::certify::{
    CERTIFY_VERSION, PROBE_SEED, Probe, ProbeOutcome, probe_features, sha256_hex,
};
use crate::profile::{Rejection, RejectionCode};

/// The **shipped interpreter Facet** (`facet-interp`), built from `facets/interp` by
/// `scripts/build-facets.sh` and committed here so the server can embed one trusted copy.
/// It is a normal gather Facet (memory/alloc/run) plus a `load_program` export; it is *our*
/// code, not community input, and it re-validates every program it is handed.
pub const INTERP_WASM: &[u8] = include_bytes!("../assets/facet_interp.wasm");

/// A DSL program's conformance certificate: the golden probe results, bound to the exact
/// bytecode by [`program_sha256`](Self::program_sha256). The registry stores it; the browser
/// replays [`probes`](Self::probes) through its own interpreter to check `preview == render`.
///
/// There is no `abiKind` (a program is always gather — features to one token per cell) and no
/// wasm [`Profile`](crate::Profile) (a program's envelope is the `mosaic-vm` caps, enforced by
/// [`mosaic_vm::validate`]); [`stride`](Self::stride) is the one structural fact a verifier
/// needs, and it is carried in every probe as well.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgramCertificate {
    /// The certificate schema version ([`CERTIFY_VERSION`]).
    pub certify_version: u32,
    /// Lowercase hex SHA-256 of the exact program bytes this certificate attests.
    pub program_sha256: String,
    /// The program's declared feature stride (features per cell).
    pub stride: u32,
    /// The golden probe results.
    pub probes: Vec<Probe>,
}

impl ProgramCertificate {
    /// Serialize to stable, pretty JSON (deterministic field/array order, so re-emitting
    /// identical input is byte-identical — the golden-diff gate depends on it).
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parse a program certificate from JSON.
    pub fn from_json(s: &str) -> Result<ProgramCertificate, serde_json::Error> {
        serde_json::from_str(s)
    }
}

/// The outcome of [`certify_program`]: an admitted program with its certificate, or a precise
/// refusal. Same two-branch shape as [`CertifyOutcome`](crate::CertifyOutcome).
pub enum ProgramCertifyOutcome {
    /// The program was admitted; here is its certificate.
    Certified(ProgramCertificate),
    /// The program was refused; here is why.
    Rejected(Rejection),
}

/// Admit and certify an untrusted DSL bytecode `program`, running it on the shared `interp`
/// Facet in the authoritative `sandbox`.
///
/// `interp` is the caller's compiled [`INTERP_WASM`] (compiled once and reused, like the
/// sandbox). Infallible given a sandbox and interpreter: every failure mode of an untrusted
/// program (malformed bytecode, traps on everything) is a [`Rejection`], never a panic.
pub fn certify_program(sandbox: &Sandbox, interp: &Facet, program: &[u8]) -> ProgramCertifyOutcome {
    // Native validation admits the program and yields its declared stride, with a precise
    // rejection code the interpreter's bare `load_program -> -1` cannot give.
    let stride = match mosaic_vm::validate(program) {
        Ok(p) => u32::from(p.stride()),
        Err(e) => return ProgramCertifyOutcome::Rejected(rejection_from_vm(e)),
    };

    let specs = program_probes(stride);
    let mut probes = Vec::with_capacity(specs.len());
    let mut produced_tokens = false;
    for (name, cols, features) in specs {
        let ncells = cols as usize;
        let outcome = match sandbox.run_program(
            interp,
            Limits::default(),
            program,
            &features,
            ncells,
            stride as usize,
        ) {
            Ok(tokens) => {
                produced_tokens = true;
                ProbeOutcome::Tokens { tokens }
            }
            Err(_) => ProbeOutcome::Trapped,
        };
        probes.push(Probe {
            name,
            stride,
            cols,
            rows: 1,
            features,
            outcome,
        });
    }

    if !produced_tokens {
        return ProgramCertifyOutcome::Rejected(Rejection {
            code: RejectionCode::NeverProducesTokens,
            message: "program produced no tokens on any probe (it trapped on every one)"
                .to_string(),
        });
    }

    ProgramCertifyOutcome::Certified(ProgramCertificate {
        certify_version: CERTIFY_VERSION,
        program_sha256: sha256_hex(program),
        stride,
        probes,
    })
}

/// The program probe suite: a few gather grids at the program's declared `stride`, each a
/// deterministic feature buffer. A program is per-cell, so every probe is one row of cells;
/// the varying widths and seeds spread the feature space (the shared generator pins 0.0 and
/// 1.0 onto slot 0 of the first two cells, so a density program always sees both endpoints).
fn program_probes(stride: u32) -> Vec<(String, u32, Vec<f32>)> {
    let mut out = Vec::new();
    for (i, &cells) in [24u32, 32, 16, 8].iter().enumerate() {
        let seed = PROBE_SEED ^ (0x51ED_2A17u64.wrapping_mul(i as u64 + 1));
        let features = probe_features(seed, cells, 1, stride);
        out.push((
            format!("program_{cells}cells_stride{stride}"),
            cells,
            features,
        ));
    }
    out
}

/// Map a `mosaic_vm::VmError` from native validation to a machine-stable [`Rejection`]. The
/// codes mirror the VM's error taxonomy so the API's "why was my program refused" is as
/// precise as the wasm profile's.
fn rejection_from_vm(e: VmError) -> Rejection {
    let (code, message): (RejectionCode, &str) = match e {
        VmError::BadMagic => (
            RejectionCode::BadMagic,
            "not a mosaic-vm bytecode program (bad magic)",
        ),
        VmError::Truncated => (RejectionCode::Truncated, "program bytes are truncated"),
        VmError::TooLarge => (RejectionCode::TooLarge, "a program section exceeds its cap"),
        VmError::BadCodepoint => (
            RejectionCode::BadCodepoint,
            "a table entry is not a valid Unicode scalar",
        ),
        VmError::BadOpcode => (
            RejectionCode::BadOpcode,
            "the program contains an unknown opcode",
        ),
        VmError::BadFeatureSlot => (
            RejectionCode::BadFeatureSlot,
            "a LOADF reads a feature slot outside the stride",
        ),
        VmError::BadParamIndex => (
            RejectionCode::BadParamIndex,
            "a LOADP reads an undeclared param",
        ),
        VmError::BadTableIndex => (
            RejectionCode::BadTableIndex,
            "a TABLE op names an undeclared table",
        ),
        VmError::StackUnderflow => (
            RejectionCode::StackUnderflow,
            "the program underflows the value stack",
        ),
        VmError::StackOverflow => (
            RejectionCode::StackOverflow,
            "the program overflows the value stack",
        ),
        VmError::BadFinalStack => (
            RejectionCode::BadFinalStack,
            "the program does not end with exactly one value",
        ),
        // `run`-time contract errors (`validate` never returns these); map defensively.
        VmError::StrideMismatch | VmError::ShortOutput | VmError::ParamCountMismatch => {
            (RejectionCode::Malformed, "malformed program")
        }
    };
    Rejection {
        code,
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_dsl::{Schema, compile};

    const ASCII_SCHEMA: Schema = Schema {
        stride: 8,
        features: &[
            ("luma", 0),
            ("grad_mag", 1),
            ("grad_dir", 2),
            ("u", 3),
            ("v", 4),
            ("r", 5),
            ("g", 6),
            ("b", 7),
        ],
        params: &[("threshold", 0.6)],
    };

    fn interp() -> (Sandbox, Facet) {
        let sandbox = Sandbox::new().unwrap();
        let facet = sandbox.compile(INTERP_WASM).unwrap();
        (sandbox, facet)
    }

    fn certified(program: &[u8]) -> ProgramCertificate {
        let (sandbox, interp) = interp();
        match certify_program(&sandbox, &interp, program) {
            ProgramCertifyOutcome::Certified(c) => c,
            ProgramCertifyOutcome::Rejected(r) => panic!("expected certification, got: {r}"),
        }
    }

    #[test]
    fn certifies_a_density_program() {
        let program = compile(r#"ramp(luma, " .:-=+*#%@")"#, &ASCII_SCHEMA).unwrap();
        let cert = certified(&program);
        assert_eq!(cert.certify_version, CERTIFY_VERSION);
        assert_eq!(cert.stride, 8);
        assert_eq!(cert.program_sha256, sha256_hex(&program));
        assert_eq!(cert.program_sha256.len(), 64);
        assert_eq!(cert.probes.len(), 4);
        for probe in &cert.probes {
            assert!(
                matches!(probe.outcome, ProbeOutcome::Tokens { .. }),
                "density program unexpectedly trapped on {}",
                probe.name
            );
            assert_eq!(probe.stride, 8);
        }
    }

    #[test]
    fn certification_is_deterministic() {
        let program = compile(r#"ramp(luma, " .:-=+*#%@")"#, &ASCII_SCHEMA).unwrap();
        let a = certified(&program);
        let b = certified(&program);
        assert_eq!(a, b, "certifying identical bytes must be identical");
    }

    #[test]
    fn certificate_round_trips_json() {
        let program = compile(r#"ramp(luma, " .:-=+*#%@")"#, &ASCII_SCHEMA).unwrap();
        let cert = certified(&program);
        let json = cert.to_json_pretty().unwrap();
        assert_eq!(ProgramCertificate::from_json(&json).unwrap(), cert);
    }

    #[test]
    fn rejects_bad_magic() {
        let (sandbox, interp) = interp();
        match certify_program(&sandbox, &interp, &[0, 0, 0, 0]) {
            ProgramCertifyOutcome::Rejected(r) => assert_eq!(r.code, RejectionCode::BadMagic),
            ProgramCertifyOutcome::Certified(_) => panic!("garbage must be rejected"),
        }
    }

    #[test]
    fn rejects_truncated_program() {
        let mut program = compile(r#"ramp(luma, " .:-=+*#%@")"#, &ASCII_SCHEMA).unwrap();
        program.truncate(program.len() - 3); // chop into the code section
        let (sandbox, interp) = interp();
        match certify_program(&sandbox, &interp, &program) {
            ProgramCertifyOutcome::Rejected(r) => assert!(
                matches!(
                    r.code,
                    RejectionCode::Truncated
                        | RejectionCode::BadFinalStack
                        | RejectionCode::BadOpcode
                ),
                "unexpected code {:?}",
                r.code
            ),
            ProgramCertifyOutcome::Certified(_) => panic!("a truncated program must be rejected"),
        }
    }
}
