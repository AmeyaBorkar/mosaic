//! # mosaic-dsl
//!
//! The **Facet DSL** (O3): a small expression language compiled to [`mosaic_vm`] bytecode.
//!
//! A Facet's per-cell logic is a single expression that reads the cell's named features
//! and the Facet's params and produces one output glyph. The expression compiles to the
//! bytecode the interpreter Facet runs in the sandbox — so an author writes text, not
//! `no_std` Rust, and the result is a shareable, inspectable program that inherits every
//! sandbox guarantee.
//!
//! ```text
//! grad_mag > 0.6 ? glyph(floor(grad_dir), "-/|\\") : ramp(luma, " .:-=+*#%@")
//! ```
//!
//! The surface is the *frontend*; the bytecode is the contract (a future visual/node
//! editor can target the same bytecode). This crate is engine-agnostic: the caller supplies
//! a [`Schema`] naming the engine's feature slots and the Facet's params, and glyph sets are
//! string literals baked into the program — no ASCII is hard-coded here.
//!
//! ## Language
//!
//! - **features / params** — bare identifiers, resolved against the [`Schema`].
//! - **numbers** — `0.6`, `9`, `-1.5`.
//! - **char literals** — `'@'` is that codepoint.
//! - **operators** — `+ - * /`, `< <= > >= == !=`, `&& || !`, unary `-`, and `c ? a : b`.
//! - **builtins** — `abs floor trunc`(1), `min max`(2), `clamp select`(3), and the glyph
//!   builtins `ramp(v, "chars")` (density: `v∈[0,1] → chars`) and `glyph(i, "chars")`
//!   (indexed: `chars[floor(i)]`, clamped).
//!
//! Every value is an `f32`; the final result is taken as a `u32` codepoint. Compilation
//! self-checks by running [`mosaic_vm::validate`] on its own output.

#![forbid(unsafe_code)]

use mosaic_vm::op;

/// The compile-time environment: the engine's feature stride and named feature slots, plus
/// the Facet's named params (with their baked-in values). Both are looked up by the bare
/// identifiers in the source.
#[derive(Debug, Clone)]
pub struct Schema<'a> {
    pub stride: u16,
    pub features: &'a [(&'a str, u16)],
    pub params: &'a [(&'a str, f32)],
}

/// A compilation failure with a byte offset into the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    pub message: String,
    pub pos: usize,
}

impl core::fmt::Display for CompileError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "at byte {}: {}", self.pos, self.message)
    }
}
impl std::error::Error for CompileError {}

fn err<T>(pos: usize, message: impl Into<String>) -> Result<T, CompileError> {
    Err(CompileError {
        message: message.into(),
        pos,
    })
}

// ---- lexer ----

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f32),
    Ident(String),
    Char(u32),
    Str(String),
    Plus,
    Minus,
    Star,
    Slash,
    Lt,
    Le,
    Gt,
    Ge,
    EqEq,
    Ne,
    AndAnd,
    OrOr,
    Bang,
    Question,
    Colon,
    Comma,
    LParen,
    RParen,
    Eof,
}

fn lex(src: &str) -> Result<Vec<(Tok, usize)>, CompileError> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        let start = i;
        match c {
            _ if c.is_ascii_whitespace() => {
                i += 1;
            }
            b'+' => {
                out.push((Tok::Plus, start));
                i += 1;
            }
            b'-' => {
                out.push((Tok::Minus, start));
                i += 1;
            }
            b'*' => {
                out.push((Tok::Star, start));
                i += 1;
            }
            b'/' => {
                out.push((Tok::Slash, start));
                i += 1;
            }
            b'?' => {
                out.push((Tok::Question, start));
                i += 1;
            }
            b':' => {
                out.push((Tok::Colon, start));
                i += 1;
            }
            b',' => {
                out.push((Tok::Comma, start));
                i += 1;
            }
            b'(' => {
                out.push((Tok::LParen, start));
                i += 1;
            }
            b')' => {
                out.push((Tok::RParen, start));
                i += 1;
            }
            b'<' => {
                if b.get(i + 1) == Some(&b'=') {
                    out.push((Tok::Le, start));
                    i += 2;
                } else {
                    out.push((Tok::Lt, start));
                    i += 1;
                }
            }
            b'>' => {
                if b.get(i + 1) == Some(&b'=') {
                    out.push((Tok::Ge, start));
                    i += 2;
                } else {
                    out.push((Tok::Gt, start));
                    i += 1;
                }
            }
            b'=' => {
                if b.get(i + 1) == Some(&b'=') {
                    out.push((Tok::EqEq, start));
                    i += 2;
                } else {
                    return err(start, "expected `==`");
                }
            }
            b'!' => {
                if b.get(i + 1) == Some(&b'=') {
                    out.push((Tok::Ne, start));
                    i += 2;
                } else {
                    out.push((Tok::Bang, start));
                    i += 1;
                }
            }
            b'&' => {
                if b.get(i + 1) == Some(&b'&') {
                    out.push((Tok::AndAnd, start));
                    i += 2;
                } else {
                    return err(start, "expected `&&`");
                }
            }
            b'|' => {
                if b.get(i + 1) == Some(&b'|') {
                    out.push((Tok::OrOr, start));
                    i += 2;
                } else {
                    return err(start, "expected `||`");
                }
            }
            b'\'' => {
                // char literal: '<char>' or an escape '\n' '\\' '\'' '\t'
                let (cp, consumed) = lex_char(&src[i..], start)?;
                out.push((Tok::Char(cp), start));
                i += consumed;
            }
            b'"' => {
                let (s, consumed) = lex_string(&src[i..], start)?;
                out.push((Tok::Str(s), start));
                i += consumed;
            }
            _ if c.is_ascii_digit() || c == b'.' => {
                let mut j = i;
                while j < b.len() && (b[j].is_ascii_digit() || b[j] == b'.') {
                    j += 1;
                }
                let text = &src[i..j];
                let n: f32 = text.parse().map_err(|_| CompileError {
                    message: format!("invalid number `{text}`"),
                    pos: start,
                })?;
                out.push((Tok::Num(n), start));
                i = j;
            }
            _ if c.is_ascii_alphabetic() || c == b'_' => {
                let mut j = i;
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                    j += 1;
                }
                out.push((Tok::Ident(src[i..j].to_string()), start));
                i = j;
            }
            _ => return err(start, format!("unexpected character `{}`", c as char)),
        }
    }
    out.push((Tok::Eof, src.len()));
    Ok(out)
}

/// Parse a `'c'` char literal (with `\n \t \\ \'` escapes), returning (codepoint, bytes
/// consumed including quotes).
fn lex_char(s: &str, pos: usize) -> Result<(u32, usize), CompileError> {
    let b = s.as_bytes();
    // b[0] == '\''
    if b.len() < 2 {
        return err(pos, "unterminated char literal");
    }
    let (cp, body_len) = if b[1] == b'\\' {
        let e = *b.get(2).ok_or_else(|| CompileError {
            message: "unterminated escape".into(),
            pos,
        })?;
        let cp = match e {
            b'n' => b'\n' as u32,
            b't' => b'\t' as u32,
            b'\\' => b'\\' as u32,
            b'\'' => b'\'' as u32,
            b'0' => 0,
            _ => return err(pos, "unknown escape in char literal"),
        };
        (cp, 3)
    } else {
        // A single UTF-8 scalar.
        let ch = s[1..].chars().next().ok_or_else(|| CompileError {
            message: "empty char literal".into(),
            pos,
        })?;
        (ch as u32, 1 + ch.len_utf8())
    };
    if b.get(body_len) != Some(&b'\'') {
        return err(pos, "expected closing `'` in char literal");
    }
    Ok((cp, body_len + 1))
}

/// Parse a `"..."` string literal (same escapes), returning (string, bytes consumed).
fn lex_string(s: &str, pos: usize) -> Result<(String, usize), CompileError> {
    let mut out = String::new();
    let mut chars = s.char_indices();
    chars.next(); // opening quote
    let mut consumed = 1;
    while let Some((_, ch)) = chars.next() {
        consumed += ch.len_utf8();
        match ch {
            '"' => return Ok((out, consumed)),
            '\\' => {
                let (_, e) = chars.next().ok_or_else(|| CompileError {
                    message: "unterminated escape in string".into(),
                    pos,
                })?;
                consumed += e.len_utf8();
                out.push(match e {
                    'n' => '\n',
                    't' => '\t',
                    '\\' => '\\',
                    '"' => '"',
                    '\'' => '\'',
                    _ => return err(pos, "unknown escape in string literal"),
                });
            }
            _ => out.push(ch),
        }
    }
    err(pos, "unterminated string literal")
}

// ---- AST ----

#[derive(Debug, Clone)]
enum Expr {
    Num(f32),
    Feature(u16),
    Param(u16),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin(u8, Box<Expr>, Box<Expr>), // opcode for the binary op
    Ne(Box<Expr>, Box<Expr>),      // compiles to EQ + NOT
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    Op1(u8, Box<Expr>),            // abs/floor/trunc
    Op2(u8, Box<Expr>, Box<Expr>), // min/max
    Clamp(Box<Expr>, Box<Expr>, Box<Expr>),
    Select(Box<Expr>, Box<Expr>, Box<Expr>),
    Ramp(Box<Expr>, u16, usize), // value, table id, table len
    Glyph(Box<Expr>, u16),       // index, table id
}

// ---- parser + table collection ----

struct Parser<'a> {
    toks: Vec<(Tok, usize)>,
    pos: usize,
    schema: &'a Schema<'a>,
    tables: Vec<Vec<u32>>,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].0
    }
    fn at(&self) -> usize {
        self.toks[self.pos].1
    }
    fn bump(&mut self) -> (Tok, usize) {
        let t = self.toks[self.pos].clone();
        self.pos += 1;
        t
    }
    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == t {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, t: &Tok, what: &str) -> Result<(), CompileError> {
        if self.eat(t) {
            Ok(())
        } else {
            err(self.at(), format!("expected {what}"))
        }
    }

    fn add_table(&mut self, s: &str) -> u16 {
        let cps: Vec<u32> = s.chars().map(|c| c as u32).collect();
        self.tables.push(cps);
        (self.tables.len() - 1) as u16
    }

    // expr := ternary
    fn parse(&mut self) -> Result<Expr, CompileError> {
        let e = self.ternary()?;
        Ok(e)
    }

    fn ternary(&mut self) -> Result<Expr, CompileError> {
        let cond = self.or()?;
        if self.eat(&Tok::Question) {
            let a = self.ternary()?;
            self.expect(&Tok::Colon, "`:` in ternary")?;
            let b = self.ternary()?;
            Ok(Expr::Ternary(Box::new(cond), Box::new(a), Box::new(b)))
        } else {
            Ok(cond)
        }
    }

    fn or(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.and()?;
        while self.eat(&Tok::OrOr) {
            let r = self.and()?;
            e = Expr::Bin(op::OR, Box::new(e), Box::new(r));
        }
        Ok(e)
    }
    fn and(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.cmp()?;
        while self.eat(&Tok::AndAnd) {
            let r = self.cmp()?;
            e = Expr::Bin(op::AND, Box::new(e), Box::new(r));
        }
        Ok(e)
    }
    fn cmp(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.add()?;
        loop {
            let opcode = match self.peek() {
                Tok::Lt => op::LT,
                Tok::Le => op::LE,
                Tok::Gt => op::GT,
                Tok::Ge => op::GE,
                Tok::EqEq => op::EQ,
                Tok::Ne => 0xFF, // sentinel -> Expr::Ne
                _ => break,
            };
            self.pos += 1;
            let r = self.add()?;
            e = if opcode == 0xFF {
                Expr::Ne(Box::new(e), Box::new(r))
            } else {
                Expr::Bin(opcode, Box::new(e), Box::new(r))
            };
        }
        Ok(e)
    }
    fn add(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.mul()?;
        loop {
            let opcode = match self.peek() {
                Tok::Plus => op::ADD,
                Tok::Minus => op::SUB,
                _ => break,
            };
            self.pos += 1;
            let r = self.mul()?;
            e = Expr::Bin(opcode, Box::new(e), Box::new(r));
        }
        Ok(e)
    }
    fn mul(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.unary()?;
        loop {
            let opcode = match self.peek() {
                Tok::Star => op::MUL,
                Tok::Slash => op::DIV,
                _ => break,
            };
            self.pos += 1;
            let r = self.unary()?;
            e = Expr::Bin(opcode, Box::new(e), Box::new(r));
        }
        Ok(e)
    }
    fn unary(&mut self) -> Result<Expr, CompileError> {
        if self.eat(&Tok::Minus) {
            Ok(Expr::Neg(Box::new(self.unary()?)))
        } else if self.eat(&Tok::Bang) {
            Ok(Expr::Not(Box::new(self.unary()?)))
        } else {
            self.primary()
        }
    }

    fn primary(&mut self) -> Result<Expr, CompileError> {
        let (tok, pos) = self.bump();
        match tok {
            Tok::Num(n) => Ok(Expr::Num(n)),
            Tok::Char(c) => Ok(Expr::Num(c as f32)),
            Tok::LParen => {
                let e = self.ternary()?;
                self.expect(&Tok::RParen, "`)`")?;
                Ok(e)
            }
            Tok::Ident(name) => {
                if self.peek() == &Tok::LParen {
                    self.call(&name, pos)
                } else {
                    self.name_ref(&name, pos)
                }
            }
            other => err(pos, format!("unexpected token in expression: {other:?}")),
        }
    }

    fn name_ref(&mut self, name: &str, pos: usize) -> Result<Expr, CompileError> {
        if let Some((_, slot)) = self.schema.features.iter().find(|(n, _)| *n == name) {
            return Ok(Expr::Feature(*slot));
        }
        if let Some(idx) = self.schema.params.iter().position(|(n, _)| *n == name) {
            return Ok(Expr::Param(idx as u16));
        }
        err(pos, format!("unknown feature or param `{name}`"))
    }

    fn arg(&mut self) -> Result<Expr, CompileError> {
        self.ternary()
    }
    fn str_arg(&mut self) -> Result<(String, usize), CompileError> {
        let (t, pos) = self.bump();
        match t {
            Tok::Str(s) => Ok((s, pos)),
            _ => err(pos, "expected a string literal (glyph set)"),
        }
    }

    fn call(&mut self, name: &str, pos: usize) -> Result<Expr, CompileError> {
        self.expect(&Tok::LParen, "`(`")?;
        let expr = match name {
            "abs" => Expr::Op1(op::ABS, Box::new(self.arg()?)),
            "floor" => Expr::Op1(op::FLOOR, Box::new(self.arg()?)),
            "trunc" => Expr::Op1(op::TRUNC, Box::new(self.arg()?)),
            "min" => {
                let a = self.arg()?;
                self.expect(&Tok::Comma, "`,`")?;
                let b = self.arg()?;
                Expr::Op2(op::MIN, Box::new(a), Box::new(b))
            }
            "max" => {
                let a = self.arg()?;
                self.expect(&Tok::Comma, "`,`")?;
                let b = self.arg()?;
                Expr::Op2(op::MAX, Box::new(a), Box::new(b))
            }
            "clamp" => {
                let x = self.arg()?;
                self.expect(&Tok::Comma, "`,`")?;
                let lo = self.arg()?;
                self.expect(&Tok::Comma, "`,`")?;
                let hi = self.arg()?;
                Expr::Clamp(Box::new(x), Box::new(lo), Box::new(hi))
            }
            "select" => {
                let c = self.arg()?;
                self.expect(&Tok::Comma, "`,`")?;
                let a = self.arg()?;
                self.expect(&Tok::Comma, "`,`")?;
                let b = self.arg()?;
                Expr::Select(Box::new(c), Box::new(a), Box::new(b))
            }
            "ramp" => {
                let v = self.arg()?;
                self.expect(&Tok::Comma, "`,`")?;
                let (s, spos) = self.str_arg()?;
                if s.is_empty() {
                    return err(spos, "ramp glyph set is empty");
                }
                let len = s.chars().count();
                let id = self.add_table(&s);
                Expr::Ramp(Box::new(v), id, len)
            }
            "glyph" => {
                let i = self.arg()?;
                self.expect(&Tok::Comma, "`,`")?;
                let (s, spos) = self.str_arg()?;
                if s.is_empty() {
                    return err(spos, "glyph set is empty");
                }
                let id = self.add_table(&s);
                Expr::Glyph(Box::new(i), id)
            }
            _ => return err(pos, format!("unknown function `{name}`")),
        };
        self.expect(&Tok::RParen, "`)` to close the call")?;
        Ok(expr)
    }
}

// ---- codegen ----

fn konst(code: &mut Vec<u8>, v: f32) {
    code.push(op::CONST);
    code.extend_from_slice(&v.to_bits().to_le_bytes());
}

fn emit(e: &Expr, code: &mut Vec<u8>) {
    match e {
        Expr::Num(n) => konst(code, *n),
        Expr::Feature(slot) => {
            code.push(op::LOADF);
            code.extend_from_slice(&slot.to_le_bytes());
        }
        Expr::Param(idx) => {
            code.push(op::LOADP);
            code.extend_from_slice(&idx.to_le_bytes());
        }
        Expr::Neg(a) => {
            emit(a, code);
            code.push(op::NEG);
        }
        Expr::Not(a) => {
            emit(a, code);
            code.push(op::NOT);
        }
        Expr::Bin(opcode, a, b) => {
            emit(a, code);
            emit(b, code);
            code.push(*opcode);
        }
        Expr::Ne(a, b) => {
            emit(a, code);
            emit(b, code);
            code.push(op::EQ);
            code.push(op::NOT);
        }
        Expr::Ternary(c, a, b) | Expr::Select(c, a, b) => {
            emit(c, code);
            emit(a, code);
            emit(b, code);
            code.push(op::SELECT);
        }
        Expr::Op1(opcode, a) => {
            emit(a, code);
            code.push(*opcode);
        }
        Expr::Op2(opcode, a, b) => {
            emit(a, code);
            emit(b, code);
            code.push(*opcode);
        }
        Expr::Clamp(x, lo, hi) => {
            emit(x, code);
            emit(lo, code);
            emit(hi, code);
            code.push(op::CLAMP);
        }
        Expr::Ramp(v, id, len) => {
            // idx = floor(clamp(v,0,1) * (len-1) + 0.5); table[idx]
            emit(v, code);
            konst(code, 0.0);
            konst(code, 1.0);
            code.push(op::CLAMP);
            konst(code, (*len as f32) - 1.0);
            code.push(op::MUL);
            konst(code, 0.5);
            code.push(op::ADD);
            code.push(op::FLOOR);
            code.push(op::TABLE);
            code.extend_from_slice(&id.to_le_bytes());
        }
        Expr::Glyph(i, id) => {
            emit(i, code);
            code.push(op::FLOOR);
            code.push(op::TABLE);
            code.extend_from_slice(&id.to_le_bytes());
        }
    }
}

/// Compile a DSL expression to a validated `mosaic-vm` bytecode program.
pub fn compile(source: &str, schema: &Schema) -> Result<Vec<u8>, CompileError> {
    let toks = lex(source)?;
    let mut p = Parser {
        toks,
        pos: 0,
        schema,
        tables: Vec::new(),
    };
    let ast = p.parse()?;
    if p.peek() != &Tok::Eof {
        return err(p.at(), "unexpected trailing input");
    }

    let mut code = Vec::new();
    emit(&ast, &mut code);
    code.push(op::END);

    // Assemble the program: magic, stride, params, tables, code.
    let mut b = Vec::new();
    b.extend_from_slice(&mosaic_vm::MAGIC.to_le_bytes());
    b.extend_from_slice(&schema.stride.to_le_bytes());
    b.extend_from_slice(&(schema.params.len() as u16).to_le_bytes());
    b.extend_from_slice(&(p.tables.len() as u16).to_le_bytes());
    for (_, v) in schema.params {
        b.extend_from_slice(&v.to_bits().to_le_bytes());
    }
    for t in &p.tables {
        b.extend_from_slice(&(t.len() as u16).to_le_bytes());
        for &c in t {
            b.extend_from_slice(&c.to_le_bytes());
        }
    }
    b.extend_from_slice(&(code.len() as u32).to_le_bytes());
    b.extend_from_slice(&code);

    // Self-check: the compiler must only ever emit programs the VM accepts.
    mosaic_vm::validate(&b).map_err(|e| CompileError {
        message: format!("internal compiler error: emitted invalid bytecode ({e:?})"),
        pos: 0,
    })?;
    Ok(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAMP: &str = " .:-=+*#%@";
    const ASCII_SCHEMA: Schema = Schema {
        stride: 3,
        features: &[("luma", 0), ("grad_mag", 1), ("grad_dir", 2)],
        params: &[("threshold", 0.6)],
    };

    fn run1(bytes: &[u8], features: &[f32], stride: usize) -> Vec<u32> {
        let prog = mosaic_vm::validate(bytes).unwrap();
        let n = features.len() / stride;
        let mut out = vec![0u32; n];
        mosaic_vm::run(&prog, features, n, stride, &mut out).unwrap();
        out
    }

    fn native_density(luma: f32) -> u32 {
        let l = luma.clamp(0.0, 1.0);
        let n = RAMP.chars().count();
        let idx = (l * (n as f32 - 1.0) + 0.5) as usize;
        RAMP.chars().nth(idx.min(n - 1)).unwrap() as u32
    }

    #[test]
    fn compiles_ramp_matching_native_density() {
        let src = r#"ramp(luma, " .:-=+*#%@")"#;
        let bytes = compile(src, &ASCII_SCHEMA).unwrap();
        let n = 128;
        let mut features = Vec::new();
        for i in 0..n {
            features.push(i as f32 / (n - 1) as f32);
            features.push(0.0);
            features.push(0.0);
        }
        let out = run1(&bytes, &features, 3);
        for (i, &tok) in out.iter().enumerate() {
            assert_eq!(tok, native_density(i as f32 / (n - 1) as f32));
        }
    }

    #[test]
    fn ternary_and_params_and_features() {
        // grad_mag > threshold ? '#' : ramp(luma, RAMP)
        let src = r#"grad_mag > threshold ? '#' : ramp(luma, " .:-=+*#%@")"#;
        let bytes = compile(src, &ASCII_SCHEMA).unwrap();
        // cell A: strong edge -> '#'; cell B: weak, mid luma -> a ramp glyph.
        let features = [0.5f32, 0.9, 0.0 /*A*/, 0.5, 0.1, 0.0 /*B*/];
        let out = run1(&bytes, &features, 3);
        assert_eq!(out[0], b'#' as u32);
        assert_eq!(out[1], native_density(0.5));
    }

    #[test]
    fn glyph_indexed_lookup_and_arithmetic() {
        // glyph(floor(luma * 3), "abcd") — luma 0,0.4,0.7,1 -> a,b,c,d(clamped)
        let src = r#"glyph(luma * 3, "abcd")"#;
        let bytes = compile(src, &ASCII_SCHEMA).unwrap();
        let features = [
            0.0f32, 0.0, 0.0, 0.4, 0.0, 0.0, 0.7, 0.0, 0.0, 1.0, 0.0, 0.0,
        ];
        let out = run1(&bytes, &features, 3);
        assert_eq!(
            out,
            vec![b'a' as u32, b'b' as u32, b'c' as u32, b'd' as u32]
        );
    }

    #[test]
    fn operator_precedence_and_grouping() {
        // 1 + 2 * 3 == 7  -> true(1.0) -> codepoint 1; (1+2)*3==9 -> also 1
        let s = Schema {
            stride: 1,
            features: &[("x", 0)],
            params: &[],
        };
        let bytes = compile("x * 2 + 1", &s).unwrap();
        let out = run1(&bytes, &[3.0], 1);
        assert_eq!(out[0], 7); // 3*2+1
        let bytes = compile("x * (2 + 1)", &s).unwrap();
        let out = run1(&bytes, &[3.0], 1);
        assert_eq!(out[0], 9); // 3*(2+1)
    }

    #[test]
    fn compile_errors_are_positioned() {
        assert!(compile("ramp(nope, \"x\")", &ASCII_SCHEMA).is_err());
        assert!(compile("luma +", &ASCII_SCHEMA).is_err());
        assert!(compile("bogus(luma)", &ASCII_SCHEMA).is_err());
        assert!(compile("luma luma", &ASCII_SCHEMA).is_err());
        // A clean, positioned message.
        let e = compile("grad_mag > ", &ASCII_SCHEMA).unwrap_err();
        assert!(e.pos > 0);
    }
}
