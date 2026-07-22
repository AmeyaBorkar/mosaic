//! # mosaic-vm
//!
//! The **Facet DSL bytecode VM** (O3): a small, safe, deterministic per-cell interpreter.
//!
//! A Facet authored in the DSL compiles to a compact bytecode *program*. This crate
//! validates that program and evaluates it once per cell — reading the cell's features and
//! the Facet's params, and producing one output codepoint — exactly the gather ABI
//! (`features → token`) every Facet implements. It is `no_std`, allocation-free, and
//! `#![forbid(unsafe_code)]`, so the *same* interpreter compiles into the wasm interpreter
//! Facet (run untrusted, in the sandbox) and into the host as a reference — one
//! implementation, no drift, the pattern already used by `glyph-atlas` and `dither`.
//!
//! ## Safety model
//!
//! Bytecode is untrusted. [`validate`] performs a full static pass — magic, bounds on
//! every feature/param/table index, a stack-effect simulation that guarantees no
//! underflow/overflow and a single result at `End`, and (because v1 is straight-line, no
//! jumps) guaranteed termination. [`run`] then evaluates a validated program without ever
//! indexing out of bounds; any residual anomaly is returned as a [`VmError`], never a
//! panic. Every operation is plain `f32` arithmetic or an IEEE round-to-integral
//! (`floor`/`trunc`) — no transcendentals, no `mul_add` — so evaluation is bit-identical
//! across the native and wasm builds (the basis of preview == render for DSL Facets).
//!
//! ## Domain-agnostic
//!
//! The VM knows nothing about ASCII. Glyph ramps and edge-glyph sets are **codepoint
//! tables in the program's constant pool**, looked up by [`op::TABLE`]; the ASCII-ness
//! lives in the compiled program, not here — so the same VM serves any glyph engine.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

/// Magic prefix, bytes `M V M 1`.
pub const MAGIC: u32 = 0x314D_564D;

/// Maximum value-stack depth. Programs whose static stack effect exceeds this are rejected.
pub const MAX_STACK: usize = 64;
/// Maximum params a program may declare.
pub const MAX_PARAMS: usize = 64;
/// Maximum codepoint tables a program may declare.
pub const MAX_TABLES: usize = 16;
/// Maximum entries in one table.
pub const MAX_TABLE_LEN: usize = 512;
/// Maximum code section length, in bytes.
pub const MAX_CODE: usize = 64 * 1024;
/// Highest valid Unicode scalar; also the cap on table entries. All valid codepoints are
/// below 2^24, so they are exact in `f32` and survive the value stack losslessly.
pub const MAX_CODEPOINT: u32 = 0x10_FFFF;

/// Opcodes. Each is one byte; a few carry little-endian operands (noted). The v1 set is
/// straight-line (no jumps), so every program terminates.
pub mod op {
    /// End: require exactly one value on the stack; the result is `top as u32` codepoint.
    pub const END: u8 = 0x00;
    /// Push an `f32` constant (operand: 4 bytes LE).
    pub const CONST: u8 = 0x01;
    /// Push `features[cell*stride + slot]` (operand: `u16` slot, LE).
    pub const LOADF: u8 = 0x02;
    /// Push `params[idx]` (operand: `u16` idx, LE).
    pub const LOADP: u8 = 0x03;
    /// Duplicate the top of stack.
    pub const DUP: u8 = 0x04;
    /// Discard the top of stack.
    pub const POP: u8 = 0x05;

    pub const ADD: u8 = 0x10;
    pub const SUB: u8 = 0x11;
    pub const MUL: u8 = 0x12;
    pub const DIV: u8 = 0x13;
    pub const NEG: u8 = 0x14;

    pub const LT: u8 = 0x20;
    pub const LE: u8 = 0x21;
    pub const GT: u8 = 0x22;
    pub const GE: u8 = 0x23;
    pub const EQ: u8 = 0x24;
    pub const NOT: u8 = 0x25;
    pub const AND: u8 = 0x26;
    pub const OR: u8 = 0x27;

    pub const MIN: u8 = 0x30;
    pub const MAX: u8 = 0x31;
    pub const ABS: u8 = 0x32;
    /// `clamp(x, lo, hi)` — pops `hi, lo, x` (x pushed first).
    pub const CLAMP: u8 = 0x33;
    pub const FLOOR: u8 = 0x34;
    pub const TRUNC: u8 = 0x35;

    /// `cond ? t : f` — pops `f, t, cond` (cond pushed first). Both branches are already
    /// evaluated (pure), so this is a branchless select.
    pub const SELECT: u8 = 0x40;

    /// Table lookup (operand: `u16` table id, LE): pop an index, push
    /// `table[clamp(index, 0, len-1)]` as an `f32` codepoint.
    pub const TABLE: u8 = 0x50;
}

/// Why a program was rejected or an evaluation could not proceed. Always a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmError {
    BadMagic,
    /// The byte stream ended inside a header field, operand, or section.
    Truncated,
    /// A declared count exceeded its cap ([`MAX_PARAMS`], [`MAX_TABLES`], …).
    TooLarge,
    /// A table entry was not a valid Unicode scalar (`> MAX_CODEPOINT`).
    BadCodepoint,
    /// An unknown opcode byte.
    BadOpcode,
    /// A `LOADF` slot was `>= stride`.
    BadFeatureSlot,
    /// A `LOADP` index was `>= n_params`.
    BadParamIndex,
    /// A `TABLE` id was `>= n_tables`.
    BadTableIndex,
    /// The static stack effect underflowed.
    StackUnderflow,
    /// The static stack effect exceeded [`MAX_STACK`].
    StackOverflow,
    /// The program did not end with exactly one value on the stack.
    BadFinalStack,
    /// `run` was given a feature buffer whose length ≠ `ncells * stride`, or a stride that
    /// disagrees with the program's declared stride.
    StrideMismatch,
    /// The output buffer was shorter than `ncells`.
    ShortOutput,
}

fn read_u16(bytes: &[u8], off: usize) -> Result<u16, VmError> {
    let b = bytes.get(off..off + 2).ok_or(VmError::Truncated)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

fn read_u32(bytes: &[u8], off: usize) -> Result<u32, VmError> {
    let b = bytes.get(off..off + 4).ok_or(VmError::Truncated)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_f32(bytes: &[u8], off: usize) -> Result<f32, VmError> {
    Ok(f32::from_bits(read_u32(bytes, off)?))
}

/// A validated, borrowed program: header offsets into the original byte slice. Produced by
/// [`validate`]; guarantees every access [`run`] makes is in bounds.
#[derive(Debug, Clone)]
pub struct Program<'a> {
    bytes: &'a [u8],
    stride: u16,
    params_off: usize,
    n_params: usize,
    table_off: [usize; MAX_TABLES],
    table_len: [usize; MAX_TABLES],
    n_tables: usize,
    code_off: usize,
    code_len: usize,
}

impl<'a> Program<'a> {
    /// Feature stride the program was compiled for.
    pub fn stride(&self) -> u16 {
        self.stride
    }

    fn param(&self, i: usize) -> Result<f32, VmError> {
        if i >= self.n_params {
            return Err(VmError::BadParamIndex);
        }
        read_f32(self.bytes, self.params_off + i * 4)
    }

    fn table_entry(&self, t: usize, i: usize) -> Result<u32, VmError> {
        if t >= self.n_tables {
            return Err(VmError::BadTableIndex);
        }
        let len = self.table_len[t];
        let idx = if len == 0 { 0 } else { i.min(len - 1) };
        if len == 0 {
            return Ok(0);
        }
        read_u32(self.bytes, self.table_off[t] + idx * 4)
    }
}

/// Validate a bytecode program end to end. On success the returned [`Program`] can be
/// evaluated by [`run`] without any out-of-bounds access.
pub fn validate(bytes: &[u8]) -> Result<Program<'_>, VmError> {
    if read_u32(bytes, 0)? != MAGIC {
        return Err(VmError::BadMagic);
    }
    let stride = read_u16(bytes, 4)?;
    let n_params = read_u16(bytes, 6)? as usize;
    let n_tables = read_u16(bytes, 8)? as usize;
    if n_params > MAX_PARAMS || n_tables > MAX_TABLES {
        return Err(VmError::TooLarge);
    }

    let params_off = 10;
    let mut off = params_off + n_params * 4;
    if bytes.len() < off {
        return Err(VmError::Truncated);
    }

    let mut table_off = [0usize; MAX_TABLES];
    let mut table_len = [0usize; MAX_TABLES];
    for (t, slot) in table_off.iter_mut().enumerate().take(n_tables) {
        let len = read_u16(bytes, off)? as usize;
        if len > MAX_TABLE_LEN {
            return Err(VmError::TooLarge);
        }
        off += 2;
        *slot = off;
        table_len[t] = len;
        // Validate every entry is a real codepoint (so f32 round-trips it losslessly).
        for i in 0..len {
            if read_u32(bytes, off + i * 4)? > MAX_CODEPOINT {
                return Err(VmError::BadCodepoint);
            }
        }
        off += len * 4;
    }

    let code_len = read_u32(bytes, off)? as usize;
    off += 4;
    if code_len > MAX_CODE {
        return Err(VmError::TooLarge);
    }
    let code_off = off;
    if bytes.len() < code_off + code_len {
        return Err(VmError::Truncated);
    }

    let program = Program {
        bytes,
        stride,
        params_off,
        n_params,
        table_off,
        table_len,
        n_tables,
        code_off,
        code_len,
    };
    validate_code(&program)?;
    Ok(program)
}

/// Walk the code once, checking opcodes/operands and simulating the stack effect so `run`
/// can never underflow, overflow, or end with the wrong number of values.
fn validate_code(p: &Program) -> Result<(), VmError> {
    let code = &p.bytes[p.code_off..p.code_off + p.code_len];
    let mut pc = 0usize;
    let mut depth: usize = 0;
    let mut ended = false;

    // pop `need` and push `produce`, tracking the high-water depth against MAX_STACK.
    macro_rules! effect {
        ($need:expr, $produce:expr) => {{
            depth = match depth.checked_sub($need) {
                Some(d) => d + $produce,
                None => return Err(VmError::StackUnderflow),
            };
            if depth > MAX_STACK {
                return Err(VmError::StackOverflow);
            }
        }};
    }

    while pc < code.len() {
        let opcode = code[pc];
        pc += 1;
        match opcode {
            op::END => {
                ended = true;
                break;
            }
            op::CONST => {
                read_f32(code, pc)?;
                pc += 4;
                effect!(0, 1);
            }
            op::LOADF => {
                let slot = read_u16(code, pc)?;
                pc += 2;
                if slot >= p.stride {
                    return Err(VmError::BadFeatureSlot);
                }
                effect!(0, 1);
            }
            op::LOADP => {
                let idx = read_u16(code, pc)? as usize;
                pc += 2;
                if idx >= p.n_params {
                    return Err(VmError::BadParamIndex);
                }
                effect!(0, 1);
            }
            op::DUP => effect!(1, 2),
            op::POP => effect!(1, 0),
            op::NEG | op::NOT | op::ABS | op::FLOOR | op::TRUNC => effect!(1, 1),
            op::ADD
            | op::SUB
            | op::MUL
            | op::DIV
            | op::LT
            | op::LE
            | op::GT
            | op::GE
            | op::EQ
            | op::AND
            | op::OR
            | op::MIN
            | op::MAX => effect!(2, 1),
            op::CLAMP | op::SELECT => effect!(3, 1),
            op::TABLE => {
                let id = read_u16(code, pc)? as usize;
                pc += 2;
                if id >= p.n_tables {
                    return Err(VmError::BadTableIndex);
                }
                effect!(1, 1);
            }
            _ => return Err(VmError::BadOpcode),
        }
    }

    if !ended || depth != 1 {
        return Err(VmError::BadFinalStack);
    }
    Ok(())
}

/// Evaluate a validated `program` once per cell, writing `ncells` output codepoints into
/// `out`. `features.len()` must equal `ncells * stride`, and `stride` must match the
/// program's declared stride.
pub fn run(
    program: &Program,
    features: &[f32],
    ncells: usize,
    stride: usize,
    out: &mut [u32],
) -> Result<(), VmError> {
    if stride != program.stride as usize {
        return Err(VmError::StrideMismatch);
    }
    if features.len() != ncells.saturating_mul(stride) {
        return Err(VmError::StrideMismatch);
    }
    if out.len() < ncells {
        return Err(VmError::ShortOutput);
    }
    let code = &program.bytes[program.code_off..program.code_off + program.code_len];
    let mut stack = [0.0f32; MAX_STACK];

    for (cell, out_cell) in out.iter_mut().enumerate().take(ncells) {
        let base = cell * stride;
        let mut sp = 0usize;
        let mut pc = 0usize;

        // Because the program is validated, these helpers never exceed MAX_STACK and never
        // underflow; they still return VmError (never panic) as defence in depth.
        macro_rules! push {
            ($v:expr) => {{
                if sp >= MAX_STACK {
                    return Err(VmError::StackOverflow);
                }
                stack[sp] = $v;
                sp += 1;
            }};
        }
        macro_rules! pop {
            () => {{
                if sp == 0 {
                    return Err(VmError::StackUnderflow);
                }
                sp -= 1;
                stack[sp]
            }};
        }

        loop {
            let opcode = *code.get(pc).ok_or(VmError::Truncated)?;
            pc += 1;
            match opcode {
                op::END => break,
                op::CONST => {
                    let v = read_f32(code, pc)?;
                    pc += 4;
                    push!(v);
                }
                op::LOADF => {
                    let slot = read_u16(code, pc)? as usize;
                    pc += 2;
                    let v = *features.get(base + slot).ok_or(VmError::BadFeatureSlot)?;
                    push!(v);
                }
                op::LOADP => {
                    let idx = read_u16(code, pc)? as usize;
                    pc += 2;
                    push!(program.param(idx)?);
                }
                op::DUP => {
                    let v = pop!();
                    push!(v);
                    push!(v);
                }
                op::POP => {
                    pop!();
                }
                op::NEG => {
                    let a = pop!();
                    push!(-a);
                }
                op::NOT => {
                    let a = pop!();
                    push!(if a == 0.0 { 1.0 } else { 0.0 });
                }
                op::ABS => {
                    let a = pop!();
                    push!(a.abs());
                }
                op::FLOOR => {
                    let a = pop!();
                    push!(libm::floorf(a));
                }
                op::TRUNC => {
                    let a = pop!();
                    push!(libm::truncf(a));
                }
                op::ADD => {
                    let b = pop!();
                    let a = pop!();
                    push!(a + b);
                }
                op::SUB => {
                    let b = pop!();
                    let a = pop!();
                    push!(a - b);
                }
                op::MUL => {
                    let b = pop!();
                    let a = pop!();
                    push!(a * b);
                }
                op::DIV => {
                    let b = pop!();
                    let a = pop!();
                    push!(a / b);
                }
                op::LT => {
                    let b = pop!();
                    let a = pop!();
                    push!(if a < b { 1.0 } else { 0.0 });
                }
                op::LE => {
                    let b = pop!();
                    let a = pop!();
                    push!(if a <= b { 1.0 } else { 0.0 });
                }
                op::GT => {
                    let b = pop!();
                    let a = pop!();
                    push!(if a > b { 1.0 } else { 0.0 });
                }
                op::GE => {
                    let b = pop!();
                    let a = pop!();
                    push!(if a >= b { 1.0 } else { 0.0 });
                }
                op::EQ => {
                    let b = pop!();
                    let a = pop!();
                    push!(if a == b { 1.0 } else { 0.0 });
                }
                op::AND => {
                    let b = pop!();
                    let a = pop!();
                    push!(if a != 0.0 && b != 0.0 { 1.0 } else { 0.0 });
                }
                op::OR => {
                    let b = pop!();
                    let a = pop!();
                    push!(if a != 0.0 || b != 0.0 { 1.0 } else { 0.0 });
                }
                op::MIN => {
                    let b = pop!();
                    let a = pop!();
                    push!(a.min(b));
                }
                op::MAX => {
                    let b = pop!();
                    let a = pop!();
                    push!(a.max(b));
                }
                op::CLAMP => {
                    let hi = pop!();
                    let lo = pop!();
                    let x = pop!();
                    push!(x.max(lo).min(hi));
                }
                op::SELECT => {
                    let f = pop!();
                    let t = pop!();
                    let cond = pop!();
                    push!(if cond != 0.0 { t } else { f });
                }
                op::TABLE => {
                    let id = read_u16(code, pc)? as usize;
                    pc += 2;
                    let idx_f = pop!();
                    // Saturating f32 -> index; clamp handled by table_entry.
                    let idx = if idx_f <= 0.0 { 0usize } else { idx_f as usize };
                    push!(program.table_entry(id, idx)? as f32);
                }
                _ => return Err(VmError::BadOpcode),
            }
        }

        if sp != 1 {
            return Err(VmError::BadFinalStack);
        }
        // Valid codepoints are non-negative and < 2^24; saturating cast is deterministic.
        let result = stack[0];
        *out_cell = if result <= 0.0 { 0 } else { result as u32 };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal helper to assemble program bytes in tests (the compiler in `mosaic-dsl` does
    /// this for real). Tests run with `std`, so `Vec` is available here.
    struct Asm {
        stride: u16,
        params: Vec<f32>,
        tables: Vec<Vec<u32>>,
        code: Vec<u8>,
    }
    impl Asm {
        fn new(stride: u16) -> Self {
            Asm {
                stride,
                params: vec![],
                tables: vec![],
                code: vec![],
            }
        }
        fn table(&mut self, t: &[u32]) -> u16 {
            self.tables.push(t.to_vec());
            (self.tables.len() - 1) as u16
        }
        fn param(&mut self, v: f32) -> u16 {
            self.params.push(v);
            (self.params.len() - 1) as u16
        }
        fn op(&mut self, b: u8) -> &mut Self {
            self.code.push(b);
            self
        }
        fn konst(&mut self, v: f32) -> &mut Self {
            self.code.push(op::CONST);
            self.code.extend_from_slice(&v.to_bits().to_le_bytes());
            self
        }
        fn loadf(&mut self, slot: u16) -> &mut Self {
            self.code.push(op::LOADF);
            self.code.extend_from_slice(&slot.to_le_bytes());
            self
        }
        fn loadp(&mut self, idx: u16) -> &mut Self {
            self.code.push(op::LOADP);
            self.code.extend_from_slice(&idx.to_le_bytes());
            self
        }
        fn table_op(&mut self, id: u16) -> &mut Self {
            self.code.push(op::TABLE);
            self.code.extend_from_slice(&id.to_le_bytes());
            self
        }
        fn finish(&mut self) -> Vec<u8> {
            self.code.push(op::END);
            let mut b = vec![];
            b.extend_from_slice(&MAGIC.to_le_bytes());
            b.extend_from_slice(&self.stride.to_le_bytes());
            b.extend_from_slice(&(self.params.len() as u16).to_le_bytes());
            b.extend_from_slice(&(self.tables.len() as u16).to_le_bytes());
            for p in &self.params {
                b.extend_from_slice(&p.to_bits().to_le_bytes());
            }
            for t in &self.tables {
                b.extend_from_slice(&(t.len() as u16).to_le_bytes());
                for &c in t {
                    b.extend_from_slice(&c.to_le_bytes());
                }
            }
            b.extend_from_slice(&(self.code.len() as u32).to_le_bytes());
            b.extend_from_slice(&self.code);
            b
        }
    }

    const RAMP: &[u32] = &[0x20, 0x2E, 0x3A, 0x2D, 0x3D, 0x2B, 0x2A, 0x23, 0x25, 0x40]; // " .:-=+*#%@"

    /// A density-ramp program: idx = floor(clamp(luma,0,1) * (len-1) + 0.5); ramp[idx].
    /// This matches facet-ramp's density_glyph exactly (clamp/mul/add/floor/lookup).
    fn density_program(stride: u16) -> Vec<u8> {
        let mut a = Asm::new(stride);
        let ramp = a.table(RAMP);
        a.loadf(0) // luma
            .konst(0.0)
            .konst(1.0)
            .op(op::CLAMP) // clamp(luma,0,1)
            .konst((RAMP.len() - 1) as f32)
            .op(op::MUL)
            .konst(0.5)
            .op(op::ADD)
            .op(op::FLOOR)
            .table_op(ramp);
        a.finish()
    }

    fn native_density(luma: f32) -> u32 {
        let l = luma.clamp(0.0, 1.0);
        let idx = (l * (RAMP.len() as f32 - 1.0) + 0.5) as usize;
        RAMP[idx.min(RAMP.len() - 1)]
    }

    #[test]
    fn density_program_matches_native_ramp() {
        let bytes = density_program(3);
        let program = validate(&bytes).unwrap();
        // Sweep luma across a stride-3 feature buffer (slots 1,2 unused here).
        let mut features = vec![];
        let n = 256;
        for i in 0..n {
            features.push(i as f32 / (n - 1) as f32); // luma
            features.push(0.0);
            features.push(0.0);
        }
        let mut out = vec![0u32; n];
        run(&program, &features, n, 3, &mut out).unwrap();
        for (i, &tok) in out.iter().enumerate() {
            let luma = i as f32 / (n - 1) as f32;
            assert_eq!(tok, native_density(luma), "luma {luma}");
        }
    }

    #[test]
    fn select_picks_branch_on_condition() {
        // program: if f0 > 0.5 { '@' } else { '.' }
        let mut a = Asm::new(1);
        a.loadf(0)
            .konst(0.5)
            .op(op::GT) // cond
            .konst(0x40 as f32) // t = '@'
            .konst(0x2E as f32) // f = '.'
            .op(op::SELECT);
        let bytes = a.finish();
        let program = validate(&bytes).unwrap();
        let features = [0.9f32, 0.1];
        let mut out = [0u32; 2];
        run(&program, &features, 2, 1, &mut out).unwrap();
        assert_eq!(out, [0x40, 0x2E]);
    }

    #[test]
    fn param_is_loaded_and_used() {
        // output = table[ floor(f0 * p0) ], p0 = 3, table = ['a','b','c','d'].
        let mut a = Asm::new(1);
        let t = a.table(&[b'a' as u32, b'b' as u32, b'c' as u32, b'd' as u32]);
        let p = a.param(3.0);
        a.loadf(0).loadp(p).op(op::MUL).op(op::FLOOR).table_op(t);
        let bytes = a.finish();
        let program = validate(&bytes).unwrap();
        let features = [0.0f32, 0.34, 0.67, 1.0];
        let mut out = [0u32; 4];
        run(&program, &features, 4, 1, &mut out).unwrap();
        // idx = floor(f*3): 0, 1, 2, and 3 (clamped to the last entry).
        assert_eq!(out, [b'a' as u32, b'b' as u32, b'c' as u32, b'd' as u32]);
    }

    #[test]
    fn determinism_reruns_identical() {
        let bytes = density_program(1);
        let program = validate(&bytes).unwrap();
        let features: Vec<f32> = (0..64).map(|i| (i as f32 * 0.017).fract()).collect();
        let mut a = vec![0u32; 64];
        let mut b = vec![0u32; 64];
        run(&program, &features, 64, 1, &mut a).unwrap();
        run(&program, &features, 64, 1, &mut b).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn rejects_bad_magic_and_truncation() {
        assert_eq!(validate(&[0, 0, 0, 0]).unwrap_err(), VmError::BadMagic);
        let mut bytes = density_program(1);
        bytes.truncate(bytes.len() - 3); // chop into the code section
        assert!(matches!(
            validate(&bytes),
            Err(VmError::Truncated | VmError::BadFinalStack | VmError::BadOpcode)
        ));
    }

    #[test]
    fn rejects_out_of_range_feature_slot() {
        // LOADF slot 5 with stride 3 must be rejected at validation.
        let mut a = Asm::new(3);
        a.loadf(5);
        let bytes = a.finish();
        assert_eq!(validate(&bytes).unwrap_err(), VmError::BadFeatureSlot);
    }

    #[test]
    fn rejects_stack_underflow_and_bad_final_stack() {
        // ADD with an empty stack -> underflow.
        let mut a = Asm::new(1);
        a.op(op::ADD);
        assert_eq!(validate(&a.finish()).unwrap_err(), VmError::StackUnderflow);
        // Two values left at END -> bad final stack.
        let mut a = Asm::new(1);
        a.konst(1.0).konst(2.0);
        assert_eq!(validate(&a.finish()).unwrap_err(), VmError::BadFinalStack);
    }

    #[test]
    fn rejects_bad_codepoint_in_table() {
        let mut a = Asm::new(1);
        let _ = a.table(&[0x11_0000]); // above MAX_CODEPOINT
        a.loadf(0).table_op(0);
        assert_eq!(validate(&a.finish()).unwrap_err(), VmError::BadCodepoint);
    }

    #[test]
    fn run_rejects_stride_and_length_mismatch() {
        let bytes = density_program(3);
        let program = validate(&bytes).unwrap();
        let mut out = [0u32; 1];
        // Wrong stride.
        assert_eq!(
            run(&program, &[0.0, 0.0, 0.0], 1, 1, &mut out).unwrap_err(),
            VmError::StrideMismatch
        );
        // features length != ncells*stride.
        assert_eq!(
            run(&program, &[0.0, 0.0], 1, 3, &mut out).unwrap_err(),
            VmError::StrideMismatch
        );
    }
}
