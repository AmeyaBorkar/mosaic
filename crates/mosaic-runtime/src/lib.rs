//! # mosaic-runtime
//!
//! The safe WASM host that executes **Facets** — untrusted, community-authored
//! methods. This crate is the security-critical core of Mosaic (decision D1): it
//! runs arbitrary compiled logic while guaranteeing, by construction, that it
//! cannot escape its box.
//!
//! A [`Sandbox`] enforces three properties on every Facet execution:
//!
//! - **Purity** — modules are instantiated with *zero* imports, so a Facet has no
//!   ambient authority: it cannot reach the network, disk, clock, or any host
//!   function. A module that even *declares* an import fails to instantiate.
//! - **Metering** — a deterministic fuel budget bounds computation; runaway code
//!   (e.g. an infinite loop) traps when the fuel is exhausted instead of hanging.
//! - **Bounded memory** — linear-memory growth is capped per execution.
//!
//! ## The Facet ABI ([`Sandbox::run_map`])
//!
//! A Facet maps a per-cell **feature buffer** to one output token per cell. The
//! engine (Tessera) computes the features; the Facet is a pure function of them.
//! Marshalling crosses the sandbox boundary through the guest's own memory:
//!
//! 1. the host asks the guest to `alloc` an input region and writes the features
//!    into it as little-endian `f32`s;
//! 2. the host `alloc`s an output region and calls
//!    `run(in_ptr, out_ptr, ncells, stride)`;
//! 3. the Facet loops over cells, reading features and writing one `u32` each;
//! 4. the host reads the `u32` output tokens back out.
//!
//! `alloc`/`run` are guest *exports*, not host imports, so purity is preserved.
//! [`Sandbox::run_i32`] is a simpler entry point used to prove the base mechanics.

#![forbid(unsafe_code)]

use anyhow::{Result, anyhow};
use wasmtime::{Config, Engine, Instance, Module, Store, StoreLimits, StoreLimitsBuilder};

/// Maximum table elements a Facet may allocate. Caps table-backed host memory
/// (funcref tables are 8 bytes/element) while still allowing normal indirect calls.
const MAX_TABLE_ELEMENTS: usize = 10_000;

/// Maximum accepted Facet module size (WASM or WAT bytes) — bounds untrusted
/// compilation cost before Cranelift ever sees the module.
const MAX_MODULE_BYTES: usize = 8 * 1024 * 1024;

/// Resource bounds for a single Facet execution.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Deterministic instruction budget. Execution traps when it reaches zero.
    pub fuel: u64,
    /// Maximum linear memory the module may allocate, in bytes.
    pub max_memory_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            fuel: 100_000_000,
            max_memory_bytes: 16 * 1024 * 1024,
        }
    }
}

/// Per-execution host state. Holds only the resource limiter — deliberately no
/// capabilities are exposed to the guest.
struct HostState {
    limits: StoreLimits,
}

/// A sandbox for compiling and executing pure, untrusted Facet modules.
///
/// One `Sandbox` owns a wasmtime [`Engine`] (with fuel metering enabled) and can
/// compile many Facets and run each under its own [`Limits`].
pub struct Sandbox {
    engine: Engine,
}

impl Sandbox {
    /// Create a sandbox with fuel metering enabled.
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
        config.consume_fuel(true);
        // Determinism across machines: canonicalize NaN payloads and forbid the
        // deliberately implementation-defined relaxed-SIMD ops.
        config.cranelift_nan_canonicalization(true);
        config.wasm_relaxed_simd(false);
        // Shrink the attack surface a pure per-cell Facet never needs. Disabling
        // threads also removes shared memory, which bypasses the memory limiter;
        // disabling multi-memory keeps a module to a single linear memory.
        config.wasm_threads(false);
        config.wasm_multi_memory(false);
        let engine = Engine::new(&config)?;
        Ok(Sandbox { engine })
    }

    /// Compile a Facet from a WASM binary or WAT text. Compilation validates the
    /// module; modules using disallowed features are rejected here.
    pub fn compile(&self, wasm_or_wat: impl AsRef<[u8]>) -> Result<Facet> {
        let bytes = wasm_or_wat.as_ref();
        if bytes.len() > MAX_MODULE_BYTES {
            return Err(anyhow!(
                "Facet module is {} bytes, exceeding the {MAX_MODULE_BYTES}-byte limit",
                bytes.len()
            ));
        }
        let module = Module::new(&self.engine, bytes)?;
        Ok(Facet { module })
    }

    /// Build a fresh, fully-limited store for one execution: zero-capability host
    /// state, a memory cap, and a fuel budget.
    fn fresh_store(&self, limits: Limits) -> Result<Store<HostState>> {
        let store_limits = StoreLimitsBuilder::new()
            .memory_size(limits.max_memory_bytes)
            .memories(1)
            .tables(1)
            .table_elements(MAX_TABLE_ELEMENTS)
            .instances(1)
            .build();
        let mut store = Store::new(
            &self.engine,
            HostState {
                limits: store_limits,
            },
        );
        store.limiter(|state| &mut state.limits);
        store.set_fuel(limits.fuel)?;
        Ok(store)
    }

    /// Instantiate `facet` under `limits` with **zero imports** and call its
    /// exported `run(i32, i32) -> i32`. Used to prove the base sandbox mechanics.
    ///
    /// Returns `Err` if the module declares any import (purity violation), if it
    /// exhausts its fuel or memory, or if it traps.
    pub fn run_i32(&self, facet: &Facet, limits: Limits, a: i32, b: i32) -> Result<i32> {
        let mut store = self.fresh_store(limits)?;
        // Zero imports: the guest receives no host functions, memories, or globals.
        let instance = Instance::new(&mut store, &facet.module, &[])?;
        let run = instance.get_typed_func::<(i32, i32), i32>(&mut store, "run")?;
        let out = run.call(&mut store, (a, b))?;
        Ok(out)
    }

    /// Shared marshalling for the Facet map ABIs: instantiate with zero imports, write
    /// `features` into the guest, allocate the `ncells`-token output, invoke `run_call`
    /// (which looks up and calls the right `run`/`run2d` export), and read the tokens
    /// back. All bounds/overflow checks and the little-endian fast paths live here so
    /// the gather and 2-D ABIs cannot drift.
    fn run_marshalled(
        &self,
        facet: &Facet,
        limits: Limits,
        features: &[f32],
        ncells: usize,
        run_call: impl FnOnce(&mut Store<HostState>, &Instance, i32, i32) -> Result<()>,
    ) -> Result<Vec<u32>> {
        // Size every buffer up front, rejecting overflow or anything beyond 32-bit
        // wasm addressing *before* allocating — an untrusted size can never drive a
        // host allocation; it becomes a clean error instead.
        let in_byte_len = checked_wasm_len(features.len(), 4)?;
        let out_byte_len = checked_wasm_len(ncells, 4)?;

        let mut store = self.fresh_store(limits)?;
        let instance = Instance::new(&mut store, &facet.module, &[])?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| anyhow!("Facet does not export 'memory'"))?;
        let alloc = instance.get_typed_func::<i32, i32>(&mut store, "alloc")?;

        // Write input features as little-endian f32 straight into guest memory. On a
        // little-endian host this is a zero-intermediate cast (`bytemuck::cast_slice`
        // is safe); big-endian keeps the portable byte loop, since the ABI is
        // documented little-endian.
        let in_ptr = alloc.call(&mut store, in_byte_len as i32)?;
        #[cfg(target_endian = "little")]
        memory.write(
            &mut store,
            in_ptr as usize,
            bytemuck::cast_slice::<f32, u8>(features),
        )?;
        #[cfg(target_endian = "big")]
        {
            let mut in_bytes = Vec::with_capacity(in_byte_len);
            for f in features {
                in_bytes.extend_from_slice(&f.to_le_bytes());
            }
            memory.write(&mut store, in_ptr as usize, &in_bytes)?;
        }

        let out_ptr = alloc.call(&mut store, out_byte_len as i32)?;
        run_call(&mut store, &instance, in_ptr, out_ptr)?;

        // Read output tokens back — directly into the u32 buffer on little-endian.
        let mut out = vec![0u32; ncells];
        #[cfg(target_endian = "little")]
        memory.read(
            &store,
            out_ptr as usize,
            bytemuck::cast_slice_mut::<u32, u8>(&mut out),
        )?;
        #[cfg(target_endian = "big")]
        {
            let mut out_bytes = vec![0u8; out_byte_len];
            memory.read(&store, out_ptr as usize, &mut out_bytes)?;
            for (dst, c) in out.iter_mut().zip(out_bytes.chunks_exact(4)) {
                *dst = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            }
        }
        Ok(out)
    }

    /// Run a gather Facet that maps a per-cell feature buffer to one `u32` output token
    /// per cell (for ASCII, a glyph codepoint). See the module docs for the ABI.
    ///
    /// `features` must be exactly `ncells * stride` values. All boundary crossings are
    /// bounds-checked by wasmtime, so a Facet returning a bogus pointer yields an
    /// `Err`, never a host panic. Zero imports and full metering apply.
    pub fn run_map(
        &self,
        facet: &Facet,
        limits: Limits,
        features: &[f32],
        ncells: usize,
        stride: usize,
    ) -> Result<Vec<u32>> {
        if stride == 0 {
            return Err(anyhow!("stride must be non-zero"));
        }
        let expected = ncells
            .checked_mul(stride)
            .ok_or_else(|| anyhow!("ncells * stride overflows"))?;
        if features.len() != expected {
            return Err(anyhow!(
                "features length {} != ncells * stride ({expected})",
                features.len()
            ));
        }
        let ncells_i32: i32 = ncells.try_into().map_err(|_| anyhow!("too many cells"))?;
        let stride_i32: i32 = stride.try_into().map_err(|_| anyhow!("stride too large"))?;

        self.run_marshalled(
            facet,
            limits,
            features,
            ncells,
            |store, instance, in_ptr, out_ptr| {
                let run =
                    instance.get_typed_func::<(i32, i32, i32, i32), ()>(&mut *store, "run")?;
                run.call(&mut *store, (in_ptr, out_ptr, ncells_i32, stride_i32))?;
                Ok(())
            },
        )
    }

    /// Run a **propagation** Facet (decision D5): like [`run_map`], but the guest
    /// exports `run2d(in_ptr, out_ptr, cols, rows, stride)` and is handed the 2-D grid
    /// shape, so it can implement feedback methods (e.g. error-diffusion dithering)
    /// whose traversal needs neighbour positions. Still one output token per cell, and
    /// the same bounds-checking, zero imports, and metering as `run_map`.
    pub fn run_map_2d(
        &self,
        facet: &Facet,
        limits: Limits,
        features: &[f32],
        cols: usize,
        rows: usize,
        stride: usize,
    ) -> Result<Vec<u32>> {
        if stride == 0 {
            return Err(anyhow!("stride must be non-zero"));
        }
        let ncells = cols
            .checked_mul(rows)
            .ok_or_else(|| anyhow!("cols * rows overflows"))?;
        let expected = ncells
            .checked_mul(stride)
            .ok_or_else(|| anyhow!("cols * rows * stride overflows"))?;
        if features.len() != expected {
            return Err(anyhow!(
                "features length {} != cols * rows * stride ({expected})",
                features.len()
            ));
        }
        let cols_i32: i32 = cols.try_into().map_err(|_| anyhow!("too many columns"))?;
        let rows_i32: i32 = rows.try_into().map_err(|_| anyhow!("too many rows"))?;
        let stride_i32: i32 = stride.try_into().map_err(|_| anyhow!("stride too large"))?;

        self.run_marshalled(
            facet,
            limits,
            features,
            ncells,
            |store, instance, in_ptr, out_ptr| {
                let run = instance
                    .get_typed_func::<(i32, i32, i32, i32, i32), ()>(&mut *store, "run2d")?;
                run.call(
                    &mut *store,
                    (in_ptr, out_ptr, cols_i32, rows_i32, stride_i32),
                )?;
                Ok(())
            },
        )
    }
}

/// Compute `count * elem_size` as a byte length usable as a 32-bit wasm address,
/// rejecting overflow or anything exceeding `i32::MAX`. Keeps host allocations
/// derived from untrusted sizes bounded.
fn checked_wasm_len(count: usize, elem_size: usize) -> Result<usize> {
    let bytes = count
        .checked_mul(elem_size)
        .ok_or_else(|| anyhow!("buffer length overflows usize"))?;
    if bytes > i32::MAX as usize {
        return Err(anyhow!(
            "buffer length {bytes} exceeds 32-bit wasm addressing"
        ));
    }
    Ok(bytes)
}

/// A compiled Facet module, ready to instantiate and run in a [`Sandbox`].
pub struct Facet {
    module: Module,
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADD_WAT: &str = r#"
        (module
          (func (export "run") (param i32 i32) (result i32)
            local.get 0
            local.get 1
            i32.add))
    "#;

    // A propagation Facet exporting `run2d`; writes cols,rows to out[0],out[1] to
    // prove run_map_2d hands the grid shape across the boundary.
    const RUN2D_WAT: &str = r#"
        (module
          (memory (export "memory") 1 1)
          (global $bump (mut i32) (i32.const 0))
          (func (export "alloc") (param $n i32) (result i32)
            (local $p i32)
            (local.set $p (global.get $bump))
            (global.set $bump (i32.add (global.get $bump) (local.get $n)))
            (local.get $p))
          (func (export "run2d")
            (param $in i32) (param $out i32) (param $cols i32) (param $rows i32) (param $stride i32)
            (i32.store (local.get $out) (local.get $cols))
            (i32.store (i32.add (local.get $out) (i32.const 4)) (local.get $rows))))
    "#;

    #[test]
    fn run_map_2d_passes_grid_shape_to_run2d() {
        let sandbox = Sandbox::new().unwrap();
        let facet = sandbox.compile(RUN2D_WAT).unwrap();
        // 3x2 grid, stride 1 -> 6 feature values; run2d writes cols,rows to out[0],[1].
        let features = vec![0.0f32; 6];
        let out = sandbox
            .run_map_2d(&facet, Limits::default(), &features, 3, 2, 1)
            .unwrap();
        assert_eq!(out, vec![3, 2, 0, 0, 0, 0]);
    }

    const INFINITE_WAT: &str = r#"
        (module
          (func (export "run") (param i32 i32) (result i32)
            (loop $l (br $l))
            unreachable))
    "#;

    const IMPORT_WAT: &str = r#"
        (module
          (import "env" "evil" (func $evil))
          (func (export "run") (param i32 i32) (result i32)
            call $evil
            i32.const 0))
    "#;

    /// A map Facet exporting the ABI (`memory`, `alloc`, `run`) that writes the
    /// codepoint 64 (`@`) for every cell — exercises the full marshalling path.
    const MAP_CONST_WAT: &str = r#"
        (module
          (memory (export "memory") 4)
          (global $bump (mut i32) (i32.const 65536))
          (func (export "alloc") (param $n i32) (result i32)
            (local $p i32)
            (local.set $p (global.get $bump))
            (global.set $bump (i32.add (global.get $bump) (local.get $n)))
            (local.get $p))
          (func (export "run") (param $in i32) (param $out i32) (param $ncells i32) (param $stride i32)
            (local $i i32)
            (block $done
              (loop $l
                (br_if $done (i32.ge_u (local.get $i) (local.get $ncells)))
                (i32.store
                  (i32.add (local.get $out) (i32.mul (local.get $i) (i32.const 4)))
                  (i32.const 64))
                (local.set $i (i32.add (local.get $i) (i32.const 1)))
                (br $l)))))
    "#;

    /// A map Facet that reads each cell's luminance (slot 0) and writes 64 (`@`)
    /// when it exceeds 0.5, else 32 (space) — proves the Facet reads its input.
    const MAP_THRESHOLD_WAT: &str = r#"
        (module
          (memory (export "memory") 4)
          (global $bump (mut i32) (i32.const 65536))
          (func (export "alloc") (param $n i32) (result i32)
            (local $p i32)
            (local.set $p (global.get $bump))
            (global.set $bump (i32.add (global.get $bump) (local.get $n)))
            (local.get $p))
          (func (export "run") (param $in i32) (param $out i32) (param $ncells i32) (param $stride i32)
            (local $i i32)
            (local $luma f32)
            (block $done
              (loop $l
                (br_if $done (i32.ge_u (local.get $i) (local.get $ncells)))
                (local.set $luma
                  (f32.load
                    (i32.add (local.get $in)
                      (i32.mul (i32.mul (local.get $i) (local.get $stride)) (i32.const 4)))))
                (i32.store
                  (i32.add (local.get $out) (i32.mul (local.get $i) (i32.const 4)))
                  (if (result i32) (f32.gt (local.get $luma) (f32.const 0.5))
                    (then (i32.const 64))
                    (else (i32.const 32))))
                (local.set $i (i32.add (local.get $i) (i32.const 1)))
                (br $l)))))
    "#;

    /// A map Facet whose `run` loops forever — must be halted by fuel.
    const MAP_INFINITE_WAT: &str = r#"
        (module
          (memory (export "memory") 1)
          (func (export "alloc") (param $n i32) (result i32) (i32.const 0))
          (func (export "run") (param i32 i32 i32 i32)
            (loop $l (br $l))))
    "#;

    #[test]
    fn pure_module_computes() {
        let sb = Sandbox::new().unwrap();
        let facet = sb.compile(ADD_WAT).unwrap();
        assert_eq!(sb.run_i32(&facet, Limits::default(), 2, 3).unwrap(), 5);
    }

    #[test]
    fn execution_is_deterministic() {
        let sb = Sandbox::new().unwrap();
        let facet = sb.compile(ADD_WAT).unwrap();
        let first = sb.run_i32(&facet, Limits::default(), 7, 35).unwrap();
        let second = sb.run_i32(&facet, Limits::default(), 7, 35).unwrap();
        assert_eq!(first, second);
        assert_eq!(first, 42);
    }

    #[test]
    fn infinite_loop_is_halted_by_fuel() {
        let sb = Sandbox::new().unwrap();
        let facet = sb.compile(INFINITE_WAT).unwrap();
        let limits = Limits {
            fuel: 100_000,
            ..Limits::default()
        };
        let result = sb.run_i32(&facet, limits, 0, 0);
        assert!(
            result.is_err(),
            "an infinite loop must exhaust its fuel and trap, not hang"
        );
    }

    #[test]
    fn module_with_imports_is_rejected() {
        // Purity: a Facet that declares any import cannot be satisfied, because we
        // instantiate with zero imports. Instantiation must fail.
        let sb = Sandbox::new().unwrap();
        let facet = sb.compile(IMPORT_WAT).unwrap();
        let result = sb.run_i32(&facet, Limits::default(), 0, 0);
        assert!(
            result.is_err(),
            "a module declaring host imports must be rejected"
        );
    }

    #[test]
    fn map_marshals_buffers_end_to_end() {
        let sb = Sandbox::new().unwrap();
        let facet = sb.compile(MAP_CONST_WAT).unwrap();
        let features = [0.1f32, 0.9, 0.5];
        let out = sb
            .run_map(&facet, Limits::default(), &features, 3, 1)
            .unwrap();
        assert_eq!(out, vec![64, 64, 64]);
    }

    #[test]
    fn map_reads_input_features() {
        let sb = Sandbox::new().unwrap();
        let facet = sb.compile(MAP_THRESHOLD_WAT).unwrap();
        let features = [0.1f32, 0.9, 0.5];
        let out = sb
            .run_map(&facet, Limits::default(), &features, 3, 1)
            .unwrap();
        assert_eq!(out, vec![32, 64, 32]);
    }

    #[test]
    fn map_rejects_mismatched_feature_length() {
        let sb = Sandbox::new().unwrap();
        let facet = sb.compile(MAP_CONST_WAT).unwrap();
        // 3 cells * stride 2 requires 6 features, not 3.
        let result = sb.run_map(&facet, Limits::default(), &[0.0, 0.0, 0.0], 3, 2);
        assert!(result.is_err());
    }

    #[test]
    fn map_respects_fuel() {
        let sb = Sandbox::new().unwrap();
        let facet = sb.compile(MAP_INFINITE_WAT).unwrap();
        let limits = Limits {
            fuel: 100_000,
            ..Limits::default()
        };
        let result = sb.run_map(&facet, limits, &[0.5f32; 4], 4, 1);
        assert!(
            result.is_err(),
            "an infinite-looping map Facet must be halted by fuel"
        );
    }

    // --- Adversarial Facets: the ABI trust boundary must reject or contain each. ---

    /// `alloc` returns a wild pointer far outside linear memory.
    const MAP_WILD_PTR_WAT: &str = r#"
        (module
          (memory (export "memory") 1)
          (func (export "alloc") (param i32) (result i32) (i32.const 2000000000))
          (func (export "run") (param i32 i32 i32 i32)))
    "#;

    /// `run` traps immediately.
    const MAP_TRAP_WAT: &str = r#"
        (module
          (memory (export "memory") 1)
          (func (export "alloc") (param i32) (result i32) (i32.const 0))
          (func (export "run") (param i32 i32 i32 i32) (unreachable)))
    "#;

    /// Grows memory far past the cap, then writes out of bounds.
    const MAP_MEMORY_BOMB_WAT: &str = r#"
        (module
          (memory (export "memory") 1)
          (func (export "alloc") (param i32) (result i32) (i32.const 0))
          (func (export "run") (param i32 i32 i32 i32)
            (drop (memory.grow (i32.const 1000000)))
            (i32.store (i32.const 1000000000) (i32.const 0))))
    "#;

    /// No `memory` export.
    const MAP_NO_MEMORY_WAT: &str = r#"
        (module
          (func (export "alloc") (param i32) (result i32) (i32.const 0))
          (func (export "run") (param i32 i32 i32 i32)))
    "#;

    /// No `alloc` export.
    const MAP_NO_ALLOC_WAT: &str = r#"
        (module
          (memory (export "memory") 1)
          (func (export "run") (param i32 i32 i32 i32)))
    "#;

    /// `run` has the wrong signature.
    const MAP_BAD_RUN_SIG_WAT: &str = r#"
        (module
          (memory (export "memory") 1)
          (func (export "alloc") (param i32) (result i32) (i32.const 0))
          (func (export "run") (param i32) (result i32) (i32.const 0)))
    "#;

    /// Declares a memory *import* — must fail purity (zero imports are granted).
    const IMPORT_MEMORY_WAT: &str = r#"
        (module
          (import "env" "mem" (memory 1))
          (func (export "run") (param i32 i32) (result i32) (i32.const 0)))
    "#;

    fn expect_map_err(wat: &str) {
        let sb = Sandbox::new().unwrap();
        let facet = sb.compile(wat).unwrap();
        let result = sb.run_map(&facet, Limits::default(), &[0.5f32; 4], 4, 1);
        assert!(result.is_err(), "expected run_map to reject this Facet");
    }

    #[test]
    fn map_rejects_wild_alloc_pointer() {
        expect_map_err(MAP_WILD_PTR_WAT);
    }

    #[test]
    fn map_rejects_trapping_run() {
        expect_map_err(MAP_TRAP_WAT);
    }

    #[test]
    fn map_contains_memory_bomb() {
        // Must error (bounded memory) and, critically, not OOM the host: if the grow
        // were honored this process would die, so reaching the assert is the proof.
        expect_map_err(MAP_MEMORY_BOMB_WAT);
    }

    #[test]
    fn map_rejects_missing_memory_export() {
        expect_map_err(MAP_NO_MEMORY_WAT);
    }

    #[test]
    fn map_rejects_missing_alloc_export() {
        expect_map_err(MAP_NO_ALLOC_WAT);
    }

    #[test]
    fn map_rejects_wrong_run_signature() {
        expect_map_err(MAP_BAD_RUN_SIG_WAT);
    }

    #[test]
    fn memory_import_is_rejected() {
        let sb = Sandbox::new().unwrap();
        let facet = sb.compile(IMPORT_MEMORY_WAT).unwrap();
        assert!(sb.run_i32(&facet, Limits::default(), 0, 0).is_err());
    }

    // --- Resource-exhaustion Facets (audit finding 1: cap tables, memories, shared). ---

    /// Declares a funcref table larger than the cap — must be denied, not allocated.
    const TABLE_BOMB_WAT: &str = r#"
        (module
          (memory (export "memory") 1)
          (table 20000 funcref)
          (func (export "alloc") (param i32) (result i32) (i32.const 0))
          (func (export "run") (param i32 i32 i32 i32)))
    "#;

    /// Declares two linear memories — must be rejected (multi-memory disabled).
    const MULTI_MEMORY_WAT: &str = r#"
        (module
          (memory (export "memory") 1)
          (memory 1)
          (func (export "alloc") (param i32) (result i32) (i32.const 0))
          (func (export "run") (param i32 i32 i32 i32)))
    "#;

    /// Declares a shared memory — must be rejected (threads disabled).
    const SHARED_MEMORY_WAT: &str = r#"
        (module
          (memory (export "memory") 1 65536 shared)
          (func (export "alloc") (param i32) (result i32) (i32.const 0))
          (func (export "run") (param i32 i32 i32 i32)))
    "#;

    #[test]
    fn map_contains_table_bomb() {
        // An oversized table must be denied at instantiation, never allocated.
        expect_map_err(TABLE_BOMB_WAT);
    }

    #[test]
    fn multi_memory_facet_is_rejected() {
        let sb = Sandbox::new().unwrap();
        assert!(sb.compile(MULTI_MEMORY_WAT).is_err());
    }

    #[test]
    fn shared_memory_facet_is_rejected() {
        let sb = Sandbox::new().unwrap();
        assert!(sb.compile(SHARED_MEMORY_WAT).is_err());
    }

    #[test]
    fn map_rejects_zero_stride() {
        let sb = Sandbox::new().unwrap();
        let facet = sb.compile(MAP_CONST_WAT).unwrap();
        assert!(sb.run_map(&facet, Limits::default(), &[], 4, 0).is_err());
    }

    #[test]
    fn oversized_module_is_rejected() {
        let sb = Sandbox::new().unwrap();
        let big = vec![0u8; MAX_MODULE_BYTES + 1];
        assert!(sb.compile(&big).is_err());
    }
}
