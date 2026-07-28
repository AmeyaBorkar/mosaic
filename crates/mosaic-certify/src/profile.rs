//! The static conformance profile: admit a Facet by structure alone.
//!
//! Every bound here is shared, by value, with `mosaic-runtime`'s sandbox limits and the
//! browser mirror in `packages/facet-abi`. [`check_profile`] inspects untrusted module
//! bytes with `wasmparser` and admits exactly what the browser admits — so a Facet the
//! server certifies is one the browser previews identically — without executing anything.

use std::fmt;

use serde::{Deserialize, Serialize};
use wasmparser::{ExternalKind, Parser, Payload};

/// Maximum accepted Facet module size, in bytes (mirrors `mosaic-runtime`'s
/// `MAX_MODULE_BYTES` and the browser's `MAX_MODULE_BYTES`).
pub const MAX_MODULE_BYTES: usize = 8 * 1024 * 1024;

/// Maximum linear-memory pages a Facet may declare as its bounded maximum (16 MiB /
/// 64 KiB). The browser enforces the declared maximum on `memory.grow`; the native
/// sandbox caps it via `StoreLimits`.
pub const MAX_MEMORY_PAGES: u32 = 256;

/// Maximum table elements a Facet may declare (mirrors the native
/// `StoreLimits::table_elements(10_000)`). A funcref table's backing store is
/// engine-side, so the linear-memory cap does not bound it.
pub const MAX_TABLE_ELEMENTS: u32 = 10_000;

/// Maximum functions a Facet may define, and the maximum size of any one body — the
/// compile-cost bounds `mosaic-runtime` applies in its structural pre-pass. A byte cap
/// alone is not a compile-cost cap (a single huge `br_table` body drives superlinear
/// Cranelift work), so both are bounded here too.
pub const MAX_FUNCTIONS: usize = 4096;
/// Maximum size of any single function body, in bytes.
pub const MAX_FUNCTION_BODY_BYTES: usize = 256 * 1024;

/// Which map entry point a Facet exports — the two module ABIs the registry admits.
/// (The DSL/program ABI is authored on the shipped interpreter and is not a
/// self-contained registry module, so it is not one of these.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbiKind {
    /// Exports `run(in, out, ncells, stride)` — one output token per cell, computed
    /// independently (`Sandbox::run_map`).
    Gather,
    /// Exports `run2d(in, out, cols, rows, stride)` — a propagation method handed the
    /// 2-D grid shape (`Sandbox::run_map_2d`).
    Propagation,
}

impl AbiKind {
    /// The name of the exported entry-point function for this ABI.
    pub const fn entry_point(self) -> &'static str {
        match self {
            AbiKind::Gather => "run",
            AbiKind::Propagation => "run2d",
        }
    }
}

/// A machine-stable reason a module is refused. Each variant corresponds to a rejection
/// point in the browser mirror, so a Facet rejected here would have been rejected there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionCode {
    /// Module byte length exceeds [`MAX_MODULE_BYTES`].
    TooLarge,
    /// The bytes are not a well-formed wasm module.
    Malformed,
    /// The module declares an import (purity is granted no ambient authority).
    Import,
    /// The module declares more than one linear memory.
    MultiMemory,
    /// The module declares a 64-bit (memory64) linear memory.
    Memory64,
    /// The module declares a shared linear memory.
    SharedMemory,
    /// A linear memory has no bounded maximum.
    UnboundedMemory,
    /// A linear memory's maximum exceeds [`MAX_MEMORY_PAGES`].
    MemoryCapExceeded,
    /// The module declares more than one table.
    MultiTable,
    /// A table's element count exceeds [`MAX_TABLE_ELEMENTS`].
    TableCapExceeded,
    /// The module declares a 64-bit table.
    Table64,
    /// The module defines more than [`MAX_FUNCTIONS`] functions.
    TooManyFunctions,
    /// A single function body exceeds [`MAX_FUNCTION_BODY_BYTES`].
    FunctionBodyTooLarge,
    /// A required ABI export (`memory` or `alloc`) is absent.
    MissingExport,
    /// A required export is present but of the wrong kind.
    BadExportKind,
    /// The module exports neither `run` nor `run2d`.
    NoEntryPoint,
    /// The module exports both `run` and `run2d` — an ambiguous ABI.
    AmbiguousAbi,
    /// The module failed to compile in the authoritative host (execution layer).
    CompileFailed,
    /// The module compiled but produced no tokens on any probe (execution layer). Shared by
    /// the wasm and DSL-program paths.
    NeverProducesTokens,

    // --- DSL program (bytecode) rejections. A published program is a `mosaic-vm` bytecode
    // blob run on the shared interpreter; these mirror `mosaic_vm::VmError` so "why was my
    // program refused" is as machine-readable as the wasm profile above. ---
    /// The program does not begin with the `mosaic-vm` magic — it is not a bytecode program.
    BadMagic,
    /// The program bytes end inside a header field, operand, table, or the code section.
    Truncated,
    /// The program contains an unknown opcode byte.
    BadOpcode,
    /// A `LOADF` reads a feature slot outside the declared stride.
    BadFeatureSlot,
    /// A `LOADP` reads a param index the program does not declare.
    BadParamIndex,
    /// A `TABLE` op names a table the program does not declare.
    BadTableIndex,
    /// A table entry is not a valid Unicode scalar value.
    BadCodepoint,
    /// The program's static stack effect underflows.
    StackUnderflow,
    /// The program's static stack effect exceeds the VM stack cap.
    StackOverflow,
    /// The program does not end with exactly one value on the stack.
    BadFinalStack,
    /// The program's declared feature stride does not match the engine it is published for
    /// (e.g. a stride-8 image program submitted for the stride-1 spectral engine).
    ProgramStrideMismatch,
    /// The submitted engine name is not one this build renders.
    UnknownEngine,
}

impl RejectionCode {
    /// A short, stable slug — the same value `#[serde]` emits — for logs and API bodies.
    pub const fn as_str(self) -> &'static str {
        match self {
            RejectionCode::TooLarge => "too_large",
            RejectionCode::Malformed => "malformed",
            RejectionCode::Import => "import",
            RejectionCode::MultiMemory => "multi_memory",
            RejectionCode::Memory64 => "memory64",
            RejectionCode::SharedMemory => "shared_memory",
            RejectionCode::UnboundedMemory => "unbounded_memory",
            RejectionCode::MemoryCapExceeded => "memory_cap_exceeded",
            RejectionCode::MultiTable => "multi_table",
            RejectionCode::TableCapExceeded => "table_cap_exceeded",
            RejectionCode::Table64 => "table64",
            RejectionCode::TooManyFunctions => "too_many_functions",
            RejectionCode::FunctionBodyTooLarge => "function_body_too_large",
            RejectionCode::MissingExport => "missing_export",
            RejectionCode::BadExportKind => "bad_export_kind",
            RejectionCode::NoEntryPoint => "no_entry_point",
            RejectionCode::AmbiguousAbi => "ambiguous_abi",
            RejectionCode::CompileFailed => "compile_failed",
            RejectionCode::NeverProducesTokens => "never_produces_tokens",
            RejectionCode::BadMagic => "bad_magic",
            RejectionCode::Truncated => "truncated",
            RejectionCode::BadOpcode => "bad_opcode",
            RejectionCode::BadFeatureSlot => "bad_feature_slot",
            RejectionCode::BadParamIndex => "bad_param_index",
            RejectionCode::BadTableIndex => "bad_table_index",
            RejectionCode::BadCodepoint => "bad_codepoint",
            RejectionCode::StackUnderflow => "stack_underflow",
            RejectionCode::StackOverflow => "stack_overflow",
            RejectionCode::BadFinalStack => "bad_final_stack",
            RejectionCode::ProgramStrideMismatch => "program_stride_mismatch",
            RejectionCode::UnknownEngine => "unknown_engine",
        }
    }
}

/// A refusal to admit or certify a Facet: a stable [`RejectionCode`] plus a
/// human-readable message. This is a value, never a panic — untrusted input that fails
/// the gate becomes a clean, serializable error the API returns to the author.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejection {
    /// The stable, machine-matchable reason.
    pub code: RejectionCode,
    /// A human-readable explanation naming the offending detail.
    pub message: String,
}

impl Rejection {
    pub(crate) fn new(code: RejectionCode, message: impl Into<String>) -> Self {
        Rejection {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for Rejection {}

/// The conformance envelope, as concrete values. Embedded in a [`Certificate`] so the
/// server and the browser agree on the bounds by value, not by coincidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    /// See [`MAX_MODULE_BYTES`].
    pub max_module_bytes: usize,
    /// See [`MAX_MEMORY_PAGES`].
    pub max_memory_pages: u32,
    /// See [`MAX_TABLE_ELEMENTS`].
    pub max_table_elements: u32,
    /// See [`MAX_FUNCTIONS`].
    pub max_functions: usize,
    /// See [`MAX_FUNCTION_BODY_BYTES`].
    pub max_function_body_bytes: usize,
}

impl Profile {
    /// The profile this build enforces.
    pub const fn current() -> Self {
        Profile {
            max_module_bytes: MAX_MODULE_BYTES,
            max_memory_pages: MAX_MEMORY_PAGES,
            max_table_elements: MAX_TABLE_ELEMENTS,
            max_functions: MAX_FUNCTIONS,
            max_function_body_bytes: MAX_FUNCTION_BODY_BYTES,
        }
    }
}

impl Default for Profile {
    fn default() -> Self {
        Profile::current()
    }
}

fn malformed(e: impl fmt::Display) -> Rejection {
    Rejection::new(RejectionCode::Malformed, format!("malformed wasm: {e}"))
}

/// Statically admit an untrusted Facet by structure alone, returning its [`AbiKind`] or a
/// precise [`Rejection`]. This is the browser-parity gate in Rust: it admits exactly what
/// `packages/facet-abi` admits — zero imports, one bounded non-shared 32-bit memory within
/// the page cap, at most one 32-bit table within the element cap, the required ABI exports,
/// and exactly one map entry point — so a Facet admitted here previews the way the server
/// renders it. It does not execute the module; that is `certify`'s job.
pub fn check_profile(bytes: &[u8]) -> Result<AbiKind, Rejection> {
    if bytes.len() > MAX_MODULE_BYTES {
        return Err(Rejection::new(
            RejectionCode::TooLarge,
            format!(
                "Facet module is {} bytes, exceeding the {MAX_MODULE_BYTES}-byte limit",
                bytes.len()
            ),
        ));
    }

    let mut memories = 0u32;
    let mut tables = 0u32;
    let mut functions = 0usize;
    let mut has_memory_export = false;
    let mut has_alloc = false;
    let mut has_run = false;
    let mut has_run2d = false;

    for payload in Parser::new(0).parse_all(bytes) {
        match payload.map_err(malformed)? {
            Payload::ImportSection(reader) => {
                // Purity: the guest is instantiated with zero imports, so any declared
                // import can never be satisfied. Reject the first one found. The section
                // is a sequence of import *groups* (wasmparser models the compact-imports
                // encoding), each of which iterates to individual imports.
                for group in reader {
                    // A non-empty group means the module imports something; the first
                    // item of the first non-empty group is enough to reject on.
                    if let Some(item) = group.map_err(malformed)?.into_iter().next() {
                        let (_, import) = item.map_err(malformed)?;
                        return Err(Rejection::new(
                            RejectionCode::Import,
                            format!(
                                "Facet must declare zero imports (purity); found {}.{}",
                                import.module, import.name
                            ),
                        ));
                    }
                }
            }
            Payload::MemorySection(reader) => {
                for mem in reader {
                    let mem = mem.map_err(malformed)?;
                    memories += 1;
                    if memories > 1 {
                        return Err(Rejection::new(
                            RejectionCode::MultiMemory,
                            "Facet must declare at most one linear memory",
                        ));
                    }
                    if mem.memory64 {
                        return Err(Rejection::new(
                            RejectionCode::Memory64,
                            "Facet memory must not be a 64-bit (memory64) memory",
                        ));
                    }
                    if mem.shared {
                        return Err(Rejection::new(
                            RejectionCode::SharedMemory,
                            "Facet memory must not be shared",
                        ));
                    }
                    match mem.maximum {
                        None => {
                            return Err(Rejection::new(
                                RejectionCode::UnboundedMemory,
                                "Facet memory must declare a bounded maximum",
                            ));
                        }
                        Some(max) if max > u64::from(MAX_MEMORY_PAGES) => {
                            return Err(Rejection::new(
                                RejectionCode::MemoryCapExceeded,
                                format!(
                                    "Facet memory maximum of {max} pages exceeds the {MAX_MEMORY_PAGES}-page (16 MiB) cap"
                                ),
                            ));
                        }
                        Some(_) => {}
                    }
                }
            }
            Payload::TableSection(reader) => {
                for table in reader {
                    let table = table.map_err(malformed)?;
                    tables += 1;
                    if tables > 1 {
                        return Err(Rejection::new(
                            RejectionCode::MultiTable,
                            "Facet must declare at most one table",
                        ));
                    }
                    let ty = table.ty;
                    if ty.table64 {
                        return Err(Rejection::new(
                            RejectionCode::Table64,
                            "Facet must not declare a 64-bit table",
                        ));
                    }
                    let over_cap = ty.initial > u64::from(MAX_TABLE_ELEMENTS)
                        || ty
                            .maximum
                            .is_some_and(|m| m > u64::from(MAX_TABLE_ELEMENTS));
                    if over_cap {
                        return Err(Rejection::new(
                            RejectionCode::TableCapExceeded,
                            format!("Facet table exceeds the {MAX_TABLE_ELEMENTS}-element cap"),
                        ));
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                functions += 1;
                if functions > MAX_FUNCTIONS {
                    return Err(Rejection::new(
                        RejectionCode::TooManyFunctions,
                        format!("Facet defines more than {MAX_FUNCTIONS} functions"),
                    ));
                }
                let range = body.range();
                let len = range.end - range.start;
                if len > MAX_FUNCTION_BODY_BYTES {
                    return Err(Rejection::new(
                        RejectionCode::FunctionBodyTooLarge,
                        format!(
                            "a Facet function body is {len} bytes, exceeding the {MAX_FUNCTION_BODY_BYTES}-byte limit"
                        ),
                    ));
                }
            }
            Payload::ExportSection(reader) => {
                for export in reader {
                    let export = export.map_err(malformed)?;
                    match export.name {
                        "memory" => {
                            if export.kind != ExternalKind::Memory {
                                return Err(bad_export_kind("memory", "memory"));
                            }
                            has_memory_export = true;
                        }
                        "alloc" => {
                            if export.kind != ExternalKind::Func {
                                return Err(bad_export_kind("alloc", "function"));
                            }
                            has_alloc = true;
                        }
                        "run" if export.kind == ExternalKind::Func => has_run = true,
                        "run2d" if export.kind == ExternalKind::Func => has_run2d = true,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    if !has_memory_export {
        return Err(Rejection::new(
            RejectionCode::MissingExport,
            "Facet does not export 'memory'",
        ));
    }
    if !has_alloc {
        return Err(Rejection::new(
            RejectionCode::MissingExport,
            "Facet does not export 'alloc'",
        ));
    }
    match (has_run, has_run2d) {
        (true, false) => Ok(AbiKind::Gather),
        (false, true) => Ok(AbiKind::Propagation),
        (false, false) => Err(Rejection::new(
            RejectionCode::NoEntryPoint,
            "Facet must export a 'run' or 'run2d' entry point (function)",
        )),
        (true, true) => Err(Rejection::new(
            RejectionCode::AmbiguousAbi,
            "Facet must export exactly one of 'run' or 'run2d', not both",
        )),
    }
}

fn bad_export_kind(name: &str, expected: &str) -> Rejection {
    Rejection::new(
        RejectionCode::BadExportKind,
        format!("Facet export '{name}' must be a {expected}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wasm(wat: &str) -> Vec<u8> {
        wat::parse_str(wat).expect("assemble wat")
    }

    /// A minimal, valid gather Facet: exports memory, alloc, run.
    const GATHER_WAT: &str = r#"
        (module
          (memory (export "memory") 1 16)
          (func (export "alloc") (param i32) (result i32) (i32.const 0))
          (func (export "run") (param i32 i32 i32 i32)))
    "#;

    /// A minimal, valid propagation Facet: exports memory, alloc, run2d.
    const PROP_WAT: &str = r#"
        (module
          (memory (export "memory") 1 16)
          (func (export "alloc") (param i32) (result i32) (i32.const 0))
          (func (export "run2d") (param i32 i32 i32 i32 i32)))
    "#;

    fn reject_code(wat: &str) -> RejectionCode {
        check_profile(&wasm(wat))
            .expect_err("expected rejection")
            .code
    }

    #[test]
    fn admits_gather_facet() {
        assert_eq!(check_profile(&wasm(GATHER_WAT)).unwrap(), AbiKind::Gather);
    }

    #[test]
    fn admits_propagation_facet() {
        assert_eq!(
            check_profile(&wasm(PROP_WAT)).unwrap(),
            AbiKind::Propagation
        );
    }

    #[test]
    fn rejects_import() {
        let wat = r#"
            (module
              (import "env" "evil" (func))
              (memory (export "memory") 1 16)
              (func (export "alloc") (param i32) (result i32) (i32.const 0))
              (func (export "run") (param i32 i32 i32 i32)))
        "#;
        assert_eq!(reject_code(wat), RejectionCode::Import);
    }

    #[test]
    fn rejects_imported_memory() {
        // An imported memory is still an import — caught by the purity check.
        let wat = r#"
            (module
              (import "env" "mem" (memory 1 16))
              (func (export "alloc") (param i32) (result i32) (i32.const 0))
              (func (export "run") (param i32 i32 i32 i32)))
        "#;
        assert_eq!(reject_code(wat), RejectionCode::Import);
    }

    #[test]
    fn rejects_multi_memory() {
        let wat = r#"
            (module
              (memory (export "memory") 1 16)
              (memory 1 1)
              (func (export "alloc") (param i32) (result i32) (i32.const 0))
              (func (export "run") (param i32 i32 i32 i32)))
        "#;
        assert_eq!(reject_code(wat), RejectionCode::MultiMemory);
    }

    #[test]
    fn rejects_memory64() {
        let wat = r#"
            (module
              (memory (export "memory") i64 1 16)
              (func (export "alloc") (param i32) (result i32) (i32.const 0))
              (func (export "run") (param i32 i32 i32 i32)))
        "#;
        assert_eq!(reject_code(wat), RejectionCode::Memory64);
    }

    #[test]
    fn rejects_shared_memory() {
        let wat = r#"
            (module
              (memory (export "memory") 1 16 shared)
              (func (export "alloc") (param i32) (result i32) (i32.const 0))
              (func (export "run") (param i32 i32 i32 i32)))
        "#;
        assert_eq!(reject_code(wat), RejectionCode::SharedMemory);
    }

    #[test]
    fn rejects_unbounded_memory() {
        let wat = r#"
            (module
              (memory (export "memory") 1)
              (func (export "alloc") (param i32) (result i32) (i32.const 0))
              (func (export "run") (param i32 i32 i32 i32)))
        "#;
        assert_eq!(reject_code(wat), RejectionCode::UnboundedMemory);
    }

    #[test]
    fn rejects_oversized_memory_maximum() {
        let wat = r#"
            (module
              (memory (export "memory") 1 300)
              (func (export "alloc") (param i32) (result i32) (i32.const 0))
              (func (export "run") (param i32 i32 i32 i32)))
        "#;
        assert_eq!(reject_code(wat), RejectionCode::MemoryCapExceeded);
    }

    #[test]
    fn rejects_multi_table() {
        let wat = r#"
            (module
              (memory (export "memory") 1 16)
              (table 1 1 funcref)
              (table 1 1 funcref)
              (func (export "alloc") (param i32) (result i32) (i32.const 0))
              (func (export "run") (param i32 i32 i32 i32)))
        "#;
        assert_eq!(reject_code(wat), RejectionCode::MultiTable);
    }

    #[test]
    fn rejects_oversized_table() {
        let wat = r#"
            (module
              (memory (export "memory") 1 16)
              (table 20000 funcref)
              (func (export "alloc") (param i32) (result i32) (i32.const 0))
              (func (export "run") (param i32 i32 i32 i32)))
        "#;
        assert_eq!(reject_code(wat), RejectionCode::TableCapExceeded);
    }

    #[test]
    fn admits_small_table() {
        let wat = r#"
            (module
              (memory (export "memory") 1 16)
              (table 10 100 funcref)
              (func (export "alloc") (param i32) (result i32) (i32.const 0))
              (func (export "run") (param i32 i32 i32 i32)))
        "#;
        assert_eq!(check_profile(&wasm(wat)).unwrap(), AbiKind::Gather);
    }

    #[test]
    fn rejects_missing_memory_export() {
        let wat = r#"
            (module
              (memory 1 16)
              (func (export "alloc") (param i32) (result i32) (i32.const 0))
              (func (export "run") (param i32 i32 i32 i32)))
        "#;
        assert_eq!(reject_code(wat), RejectionCode::MissingExport);
    }

    #[test]
    fn rejects_missing_alloc_export() {
        let wat = r#"
            (module
              (memory (export "memory") 1 16)
              (func (export "run") (param i32 i32 i32 i32)))
        "#;
        assert_eq!(reject_code(wat), RejectionCode::MissingExport);
    }

    #[test]
    fn rejects_no_entry_point() {
        let wat = r#"
            (module
              (memory (export "memory") 1 16)
              (func (export "alloc") (param i32) (result i32) (i32.const 0)))
        "#;
        assert_eq!(reject_code(wat), RejectionCode::NoEntryPoint);
    }

    #[test]
    fn rejects_ambiguous_abi() {
        let wat = r#"
            (module
              (memory (export "memory") 1 16)
              (func (export "alloc") (param i32) (result i32) (i32.const 0))
              (func (export "run") (param i32 i32 i32 i32))
              (func (export "run2d") (param i32 i32 i32 i32 i32)))
        "#;
        assert_eq!(reject_code(wat), RejectionCode::AmbiguousAbi);
    }

    #[test]
    fn rejects_wrong_kind_alloc_export() {
        // `alloc` exported as a global, not a function.
        let wat = r#"
            (module
              (memory (export "memory") 1 16)
              (global (export "alloc") i32 (i32.const 0))
              (func (export "run") (param i32 i32 i32 i32)))
        "#;
        assert_eq!(reject_code(wat), RejectionCode::BadExportKind);
    }

    #[test]
    fn rejects_malformed_bytes() {
        // Valid magic + version, then garbage — parses as a module header but fails.
        let bytes = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0xff, 0xff];
        assert_eq!(
            check_profile(&bytes).unwrap_err().code,
            RejectionCode::Malformed
        );
    }

    #[test]
    fn rejects_oversized_module() {
        let big = vec![0u8; MAX_MODULE_BYTES + 1];
        assert_eq!(
            check_profile(&big).unwrap_err().code,
            RejectionCode::TooLarge
        );
    }

    #[test]
    fn profile_current_matches_constants() {
        let p = Profile::current();
        assert_eq!(p.max_module_bytes, MAX_MODULE_BYTES);
        assert_eq!(p.max_memory_pages, MAX_MEMORY_PAGES);
        assert_eq!(p.max_table_elements, MAX_TABLE_ELEMENTS);
    }
}
