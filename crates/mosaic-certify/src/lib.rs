//! # mosaic-certify
//!
//! The **authoritative conformance gate**. The browser host (`@mosaic/facet-abi`)
//! enforces the static conformance profile "user side" so a preview never runs a
//! Facet the server would refuse; this crate is the *server side* — the authority
//! that admits a Facet only if it fits that same profile, and then **certifies** it.
//!
//! Two layers, matching how a Facet is admitted:
//!
//! - [`check_profile`] — a fast, execution-free structural gate over untrusted module
//!   bytes. It admits *exactly* what the browser mirror admits (zero imports; one
//!   bounded, non-shared, 32-bit linear memory within the page cap; at most one table
//!   within the element cap; the required ABI exports; a single map entry point) and
//!   reports the Facet's [`AbiKind`], or a precise [`Rejection`]. This is what the
//!   render path runs on an inline Facet before executing it.
//! - `certify` (added in the execution layer) — runs the admitted Facet through the
//!   proven native host ([`mosaic_runtime`]) over a deterministic probe suite and emits
//!   a [`Certificate`]: the golden `(features -> tokens)` vectors the browser must
//!   reproduce, making `preview == render` (decision D9) a checked property for *any*
//!   certified Facet, not just the shipped ones.
//! - [`certify_program`] — the same admission and probing for a **DSL program** (`mosaic-vm`
//!   bytecode) rather than a self-contained module: it validates the bytecode natively, then
//!   probes it through the shipped interpreter Facet ([`INTERP_WASM`]) and emits a
//!   [`ProgramCertificate`]. This is how an authored DSL Facet is admitted to the registry.
//!
//! The profile constants here are the single numeric contract shared with
//! `mosaic-runtime`'s sandbox limits and the browser mirror; they are recorded in the
//! certificate so both sides agree by value, not by coincidence.

#![forbid(unsafe_code)]

mod certify;
mod profile;
mod program;

pub use certify::{CERTIFY_VERSION, Certificate, CertifyOutcome, Probe, ProbeOutcome, certify};
pub use profile::{
    AbiKind, MAX_FUNCTION_BODY_BYTES, MAX_FUNCTIONS, MAX_MEMORY_PAGES, MAX_MODULE_BYTES,
    MAX_TABLE_ELEMENTS, Profile, Rejection, RejectionCode, check_profile,
};
pub use program::{INTERP_WASM, ProgramCertificate, ProgramCertifyOutcome, certify_program};
