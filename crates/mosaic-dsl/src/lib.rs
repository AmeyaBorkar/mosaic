//! # mosaic-dsl
//!
//! **Glint** (O3) — the Facet authoring language: a small expression language compiled to
//! [`mosaic_vm`] bytecode. (`mosaic-dsl` is the crate; *Glint* is the language it compiles.)
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
//! - **let** — `let NAME = EXPR; BODY` names a reusable subexpression, in scope over `BODY`
//!   (and shadowing a feature/param of the same name). Purely a frontend convenience — a
//!   binding is a *shared* subexpression, emitted where used, so it never adds VM power.
//! - **builtins** — `abs floor trunc`(1), `min max`(2), `clamp select`(3); the curve helpers
//!   `mix(a, b, t)`, `remap(x, inLo, inHi, outLo, outHi)`, `smoothstep(e0, e1, x)` (which
//!   lower to the arithmetic ops above); `noise(x, y)` — a deterministic hash of two
//!   coordinates to `[0, 1)` for stipple / grain / hand-dither texture (feed it the position
//!   slots `u`/`v`); and the glyph builtins `ramp(v, "chars")` (density: `v∈[0,1] → chars`)
//!   and `glyph(i, "chars")` (indexed: `chars[floor(i)]`, clamped).
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
    Eq,
    Semi,
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
            b';' => {
                out.push((Tok::Semi, start));
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
                    out.push((Tok::Eq, start)); // a `let` binding (`==` is comparison)
                    i += 1;
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
            _ => {
                // Decode the real char (i is at a char boundary — all prior tokens are
                // ASCII), not `c as char` which would mangle a UTF-8 lead byte to Latin-1
                // and name a character the author never typed. Audit L5.
                let ch = src[i..].chars().next().unwrap_or(c as char);
                return err(start, format!("unexpected character `{ch}`"));
            }
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
    /// A reference to a shared subexpression in `Parser::bindings` — a `let` binding, or an
    /// internal binding introduced when desugaring a builtin whose argument is used more than
    /// once. Emitted by re-emitting the bound expression's code. Sharing (rather than cloning
    /// the source tree) keeps the AST linear, so nested desugarings can't blow it up
    /// exponentially; `emit` still caps the *emitted* bytes at the VM's code limit.
    LetRef(usize),
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

/// Maximum expression nesting depth. The recursive-descent parser (and the recursive
/// `emit` and `Drop` of the AST) run host-side, outside the sandbox, so an untrusted
/// source of deeply nested parens or unary operators would otherwise overflow the native
/// stack — an uncatchable abort, not a `CompileError`. 256 is far beyond any real Facet.
const MAX_PARSE_DEPTH: usize = 256;

struct Parser<'a> {
    toks: Vec<(Tok, usize)>,
    pos: usize,
    schema: &'a Schema<'a>,
    tables: Vec<Vec<u32>>,
    depth: usize,
    /// Shared subexpressions, referenced by [`Expr::LetRef`]. A binding may only reference
    /// earlier bindings (they are pushed in source order and resolved lexically), so the
    /// reference graph is acyclic and `emit` always terminates.
    bindings: Vec<Expr>,
    /// Lexical scope: in-scope `let` names → their binding index. Searched newest-first, so a
    /// later `let` shadows an earlier one (or a feature/param of the same name).
    scopes: Vec<(String, usize)>,
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

    // expr := let* ternary
    fn parse(&mut self) -> Result<Expr, CompileError> {
        self.expr()
    }

    /// An expression, optionally prefixed by `let` bindings.
    fn expr(&mut self) -> Result<Expr, CompileError> {
        if matches!(self.peek(), Tok::Ident(n) if n == "let") {
            self.parse_let()
        } else {
            self.ternary()
        }
    }

    /// `let NAME = EXPR ; BODY` — name a reusable subexpression, in scope over BODY only.
    /// The value is stored once in `bindings`; each use in BODY becomes an [`Expr::LetRef`], so
    /// it is evaluated where used (shared in the AST, expanded at emit) rather than recomputed
    /// by cloning the source tree — which is what keeps a chain of bindings from exploding.
    fn parse_let(&mut self) -> Result<Expr, CompileError> {
        // `let` bodies recurse here without passing through `unary`, so guard depth here too.
        self.depth += 1;
        if self.depth > MAX_PARSE_DEPTH {
            self.depth -= 1;
            return err(self.at(), "expression nests too deeply");
        }
        self.bump(); // `let`
        let (name, npos) = match self.bump() {
            (Tok::Ident(n), p) => (n, p),
            (_, p) => {
                self.depth -= 1;
                return err(p, "expected a name after `let`");
            }
        };
        if name == "let" {
            self.depth -= 1;
            return err(npos, "`let` is a reserved word");
        }
        self.expect(&Tok::Eq, "`=` after the let name")?;
        let value = self.expr()?;
        self.expect(&Tok::Semi, "`;` after the let value")?;
        let idx = self.bindings.len();
        self.bindings.push(value);
        self.scopes.push((name, idx));
        let body = self.expr()?;
        self.scopes.pop();
        self.depth -= 1;
        Ok(body)
    }

    /// Store `e` as a shared binding and return a cheap `LetRef` leaf to it. Used to desugar a
    /// builtin whose argument is consumed more than once, so the argument is emitted once per
    /// use but never *cloned* — a nested desugaring can't explode the AST.
    fn share(&mut self, e: Expr) -> Expr {
        let idx = self.bindings.len();
        self.bindings.push(e);
        Expr::LetRef(idx)
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
        // Every descent level (paren nesting and unary chains alike) passes through here
        // exactly once, so guarding depth here bounds the whole parser's stack use.
        self.depth += 1;
        if self.depth > MAX_PARSE_DEPTH {
            self.depth -= 1;
            return err(self.at(), "expression nests too deeply");
        }
        let r = self.unary_inner();
        self.depth -= 1;
        r
    }
    fn unary_inner(&mut self) -> Result<Expr, CompileError> {
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
                let e = self.expr()?;
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
        if let Some(&(_, idx)) = self.scopes.iter().rev().find(|(n, _)| n == name) {
            return Ok(Expr::LetRef(idx));
        }
        if let Some((_, slot)) = self.schema.features.iter().find(|(n, _)| *n == name) {
            return Ok(Expr::Feature(*slot));
        }
        if let Some(idx) = self.schema.params.iter().position(|(n, _)| *n == name) {
            return Ok(Expr::Param(idx as u16));
        }
        err(pos, format!("unknown feature or param `{name}`"))
    }

    fn arg(&mut self) -> Result<Expr, CompileError> {
        self.expr()
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
            // Convenience curves that lower to existing ops. Any argument used more than once
            // is `share`d, so a nested `mix`/`remap`/`smoothstep` cannot expand exponentially.
            "mix" => {
                let a = self.arg()?;
                self.expect(&Tok::Comma, "`,`")?;
                let b = self.arg()?;
                self.expect(&Tok::Comma, "`,`")?;
                let t = self.arg()?;
                // a + (b - a) * t  — `a` is used twice.
                let a = self.share(a);
                bin(op::ADD, a.clone(), bin(op::MUL, bin(op::SUB, b, a), t))
            }
            "remap" => {
                let x = self.arg()?;
                self.expect(&Tok::Comma, "`,`")?;
                let in_lo = self.arg()?;
                self.expect(&Tok::Comma, "`,`")?;
                let in_hi = self.arg()?;
                self.expect(&Tok::Comma, "`,`")?;
                let out_lo = self.arg()?;
                self.expect(&Tok::Comma, "`,`")?;
                let out_hi = self.arg()?;
                // t = clamp((x - in_lo) / (in_hi - in_lo), 0, 1);  out_lo + (out_hi - out_lo)*t
                let in_lo = self.share(in_lo); // used twice
                let t = clamp01(bin(
                    op::DIV,
                    bin(op::SUB, x, in_lo.clone()),
                    bin(op::SUB, in_hi, in_lo),
                ));
                let out_lo = self.share(out_lo); // used twice
                bin(
                    op::ADD,
                    out_lo.clone(),
                    bin(op::MUL, bin(op::SUB, out_hi, out_lo), t),
                )
            }
            "smoothstep" => {
                let e0 = self.arg()?;
                self.expect(&Tok::Comma, "`,`")?;
                let e1 = self.arg()?;
                self.expect(&Tok::Comma, "`,`")?;
                let x = self.arg()?;
                // t = clamp((x - e0) / (e1 - e0), 0, 1);  t*t*(3 - 2t)
                let e0 = self.share(e0); // used twice
                let t = self.share(clamp01(bin(
                    op::DIV,
                    bin(op::SUB, x, e0.clone()),
                    bin(op::SUB, e1, e0),
                ))); // used three times
                bin(
                    op::MUL,
                    bin(op::MUL, t.clone(), t.clone()),
                    bin(op::SUB, num(3.0), bin(op::MUL, num(2.0), t)),
                )
            }
            // Deterministic hash noise → the VM's HASH op. Two args, one result, so it shares
            // Op2's shape (like `min`/`max`) and needs no new codegen or Expr variant.
            "noise" => {
                let x = self.arg()?;
                self.expect(&Tok::Comma, "`,`")?;
                let y = self.arg()?;
                Expr::Op2(op::HASH, Box::new(x), Box::new(y))
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

fn bin(opcode: u8, a: Expr, b: Expr) -> Expr {
    Expr::Bin(opcode, Box::new(a), Box::new(b))
}

fn num(v: f32) -> Expr {
    Expr::Num(v)
}

fn clamp01(e: Expr) -> Expr {
    Expr::Clamp(
        Box::new(e),
        Box::new(Expr::Num(0.0)),
        Box::new(Expr::Num(1.0)),
    )
}

/// Emit bytecode for `e`, expanding [`Expr::LetRef`]s against `binds`. Fallible only to enforce
/// a hard cap on the *emitted* size: `let`/builtin sharing keeps the AST linear, but a
/// pathological nest of shared references could still try to expand to exponentially many bytes,
/// so bail cleanly at the VM's code limit instead of building a giant buffer.
fn emit(e: &Expr, code: &mut Vec<u8>, binds: &[Expr]) -> Result<(), CompileError> {
    if code.len() >= mosaic_vm::MAX_CODE {
        return err(
            0,
            "compiled program is too large — reduce nesting or reuse subexpressions with `let`",
        );
    }
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
        Expr::LetRef(i) => emit(&binds[*i], code, binds)?,
        Expr::Neg(a) => {
            emit(a, code, binds)?;
            code.push(op::NEG);
        }
        Expr::Not(a) => {
            emit(a, code, binds)?;
            code.push(op::NOT);
        }
        Expr::Bin(opcode, a, b) => {
            emit(a, code, binds)?;
            emit(b, code, binds)?;
            code.push(*opcode);
        }
        Expr::Ne(a, b) => {
            emit(a, code, binds)?;
            emit(b, code, binds)?;
            code.push(op::EQ);
            code.push(op::NOT);
        }
        Expr::Ternary(c, a, b) | Expr::Select(c, a, b) => {
            emit(c, code, binds)?;
            emit(a, code, binds)?;
            emit(b, code, binds)?;
            code.push(op::SELECT);
        }
        Expr::Op1(opcode, a) => {
            emit(a, code, binds)?;
            code.push(*opcode);
        }
        Expr::Op2(opcode, a, b) => {
            emit(a, code, binds)?;
            emit(b, code, binds)?;
            code.push(*opcode);
        }
        Expr::Clamp(x, lo, hi) => {
            emit(x, code, binds)?;
            emit(lo, code, binds)?;
            emit(hi, code, binds)?;
            code.push(op::CLAMP);
        }
        Expr::Ramp(v, id, len) => {
            // idx = floor(clamp(v,0,1) * (len-1) + 0.5); table[idx]
            emit(v, code, binds)?;
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
            emit(i, code, binds)?;
            code.push(op::FLOOR);
            code.push(op::TABLE);
            code.extend_from_slice(&id.to_le_bytes());
        }
    }
    Ok(())
}

/// Compile a DSL expression to a validated `mosaic-vm` bytecode program.
pub fn compile(source: &str, schema: &Schema) -> Result<Vec<u8>, CompileError> {
    let toks = lex(source)?;
    let mut p = Parser {
        toks,
        pos: 0,
        schema,
        tables: Vec::new(),
        depth: 0,
        bindings: Vec::new(),
        scopes: Vec::new(),
    };
    let ast = p.parse()?;
    if p.peek() != &Tok::Eof {
        return err(p.at(), "unexpected trailing input");
    }

    let mut code = Vec::new();
    emit(&ast, &mut code, &p.bindings)?;
    code.push(op::END);

    // Reject anything that would truncate in the u16 header fields below. A glyph set
    // of >65 535 chars would otherwise write length 0 while still emitting its full
    // payload, desyncing the program's sections so the VM reads the code section out of
    // the table bytes — source and executed bytecode would diverge. Report the VM's own
    // admission limits as clean user errors instead of the opaque self-check failure.
    if schema.params.len() > mosaic_vm::MAX_PARAMS {
        return err(
            0,
            format!(
                "schema declares {} parameters, exceeding the maximum of {}",
                schema.params.len(),
                mosaic_vm::MAX_PARAMS
            ),
        );
    }
    if p.tables.len() > mosaic_vm::MAX_TABLES {
        return err(
            0,
            format!(
                "program uses {} glyph sets, exceeding the maximum of {}",
                p.tables.len(),
                mosaic_vm::MAX_TABLES
            ),
        );
    }
    if let Some(t) = p.tables.iter().find(|t| t.len() > mosaic_vm::MAX_TABLE_LEN) {
        return err(
            0,
            format!(
                "a glyph set has {} characters, exceeding the maximum of {}",
                t.len(),
                mosaic_vm::MAX_TABLE_LEN
            ),
        );
    }

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

    #[test]
    fn mix_remap_smoothstep_lower_correctly() {
        // mix(60, 80, 0.5) = 70.
        let b = compile("mix(60.0, 80.0, 0.5)", &ASCII_SCHEMA).unwrap();
        assert_eq!(run1(&b, &[0.0, 0.0, 0.0], 3), vec![70]);

        // remap(luma, 0,1, 0,100) rescales and clamps outside [0,1].
        let b = compile("remap(luma, 0.0, 1.0, 0.0, 100.0)", &ASCII_SCHEMA).unwrap();
        let feats = [0.5, 0.0, 0.0, 2.0, 0.0, 0.0, -1.0, 0.0, 0.0];
        assert_eq!(run1(&b, &feats, 3), vec![50, 100, 0]);

        // smoothstep(0,1,luma)*100: 0 -> 0, 0.5 -> 50, 1 -> 100 (the S-curve is 0.5 at the midpoint).
        let b = compile("smoothstep(0.0, 1.0, luma) * 100.0", &ASCII_SCHEMA).unwrap();
        let feats = [0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 1.0, 0.0, 0.0];
        assert_eq!(run1(&b, &feats, 3), vec![0, 50, 100]);
    }

    #[test]
    fn let_binds_and_scopes() {
        // A binding used twice equals writing it inline.
        let a = compile("let d = luma + luma; d + d", &ASCII_SCHEMA).unwrap();
        let inline = compile("(luma + luma) + (luma + luma)", &ASCII_SCHEMA).unwrap();
        let feats = [10.0, 0.0, 0.0];
        assert_eq!(run1(&a, &feats, 3), run1(&inline, &feats, 3));
        assert_eq!(run1(&a, &feats, 3), vec![40]);

        // A later binding can reference an earlier one.
        let c = compile("let a = 5.0; let b = a + 1.0; b + a", &ASCII_SCHEMA).unwrap();
        assert_eq!(run1(&c, &[0.0, 0.0, 0.0], 3), vec![11]);

        // A binding shadows a feature of the same name within its scope (lexical scoping).
        let d = compile("let luma = 3.0; luma", &ASCII_SCHEMA).unwrap();
        assert_eq!(run1(&d, &[0.9, 0.0, 0.0], 3), vec![3]);
    }

    #[test]
    fn let_reads_cleanly_in_a_facet() {
        // The edge-or-density Facet, made readable with a binding.
        let src = r#"let edge = grad_mag; edge > threshold ? glyph(clamp(grad_dir + 2.0, 0, 3), "-/|\\") : ramp(luma, " .:-=+*#%@")"#;
        let b = compile(src, &ASCII_SCHEMA).unwrap();
        let feats = [0.5, 0.9, 0.0, 0.5, 0.1, 0.0]; // strong edge, then weak edge
        let out = run1(&b, &feats, 3);
        assert_ne!(
            out[0], out[1],
            "the two branches must produce different glyphs"
        );
    }

    #[test]
    fn pathological_let_expansion_is_a_clean_error() {
        // Doubling bindings would expand to ~2^30 bytes; the emit guard rejects it cleanly
        // (no panic, no OOM) instead of building a giant buffer. The AST stays linear.
        let mut src = String::from("let a0 = luma;\n");
        for i in 1..=30 {
            src.push_str(&format!("let a{i} = a{prev} + a{prev};\n", prev = i - 1));
        }
        src.push_str("a30");
        assert!(
            compile(&src, &ASCII_SCHEMA).is_err(),
            "a 2^30-byte expansion must be rejected"
        );
    }

    #[test]
    fn existing_programs_still_compile() {
        // Nothing that predates let/curves changes: the classic density Facet is unaffected.
        let b = compile(r#"ramp(luma, " .:-=+*#%@")"#, &ASCII_SCHEMA).unwrap();
        assert!(mosaic_vm::validate(&b).is_ok());
    }

    #[test]
    fn position_features_resolve_and_run() {
        // The `ascii` engine's spatial slots `u` (3) and `v` (4) resolve by name and load
        // independently. This stride-5 schema exercises the position slots; the compiler is
        // engine-agnostic, so the real ascii vocabulary being larger (stride 8) is immaterial.
        const POS_SCHEMA: Schema = Schema {
            stride: 5,
            features: &[
                ("luma", 0),
                ("grad_mag", 1),
                ("grad_dir", 2),
                ("u", 3),
                ("v", 4),
            ],
            params: &[],
        };
        let b = compile("floor(u * 100.0) + floor(v * 10.0)", &POS_SCHEMA).unwrap();
        // One cell: u=0.25 (slot 3), v=0.75 (slot 4). floor(25.0)+floor(7.5) = 25+7 = 32.
        let feats = [0.0, 0.0, 0.0, 0.25, 0.75];
        assert_eq!(run1(&b, &feats, 5), vec![32]);
    }

    #[test]
    fn noise_is_spatial_deterministic_texture() {
        // `noise(u, v)` reads the position slots and hashes them to [0, 1). Scaled up so the
        // codepoint output is observable, `floor(noise(u, v) * 1000)` lands in 0..=999, varies
        // across the grid, and is stable on re-run — a pure function of its two arguments.
        const POS_SCHEMA: Schema = Schema {
            stride: 5,
            features: &[
                ("luma", 0),
                ("grad_mag", 1),
                ("grad_dir", 2),
                ("u", 3),
                ("v", 4),
            ],
            params: &[],
        };
        let b = compile("floor(noise(u, v) * 1000.0)", &POS_SCHEMA).unwrap();
        let mut feats = Vec::new();
        for row in 0..8u32 {
            for col in 0..8u32 {
                let u = (col as f32 + 0.5) / 8.0;
                let v = (row as f32 + 0.5) / 8.0;
                feats.extend_from_slice(&[0.0, 0.0, 0.0, u, v]);
            }
        }
        let out = run1(&b, &feats, 5);
        assert!(out.iter().all(|&t| t <= 999));
        let distinct: std::collections::BTreeSet<u32> = out.iter().copied().collect();
        assert!(
            distinct.len() > 20,
            "noise texture is degenerate: {} distinct",
            distinct.len()
        );
        assert_eq!(out, run1(&b, &feats, 5), "noise must be deterministic");
        // Cell 10 (row 1, col 2) in isolation hashes identically — a pure function of (u, v),
        // independent of the surrounding cells.
        let (u, v) = (2.5 / 8.0, 1.5 / 8.0);
        let single = run1(&b, &[0.0, 0.0, 0.0, u, v], 5);
        assert_eq!(single[0], out[10]);
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
    fn deeply_nested_source_is_a_clean_error_not_a_stack_overflow() {
        // Untrusted source compiled host-side: deep nesting must be a CompileError, never
        // a native stack-overflow abort. Audit L1.
        let deep = format!("{}1{}", "(".repeat(1000), ")".repeat(1000));
        let e = compile(&deep, &ASCII_SCHEMA).unwrap_err();
        assert!(e.message.contains("nests too deeply"), "got: {}", e.message);
        // Unary chains funnel through the same guard.
        let unary = format!("{}1", "-".repeat(1000));
        assert!(compile(&unary, &ASCII_SCHEMA).is_err());
    }

    #[test]
    fn non_ascii_error_names_the_real_character() {
        // The diagnostic must name '×' (U+00D7), not the Latin-1 mojibake of its UTF-8
        // lead byte. Audit L5.
        let e = compile("luma × 2", &ASCII_SCHEMA).unwrap_err();
        assert!(e.message.contains('×'), "got: {}", e.message);
    }

    #[test]
    fn oversized_glyph_set_is_a_clean_error_not_a_truncation() {
        // A glyph set longer than MAX_TABLE_LEN must be a clean, human-readable error,
        // never a silent u16 truncation that desyncs the program's sections. Audit M8.
        let big = "x".repeat(mosaic_vm::MAX_TABLE_LEN + 1);
        let src = format!("ramp(luma, \"{big}\")");
        let e = compile(&src, &ASCII_SCHEMA).unwrap_err();
        assert!(
            e.message.contains("glyph set"),
            "expected a glyph-set-too-large error, got: {}",
            e.message
        );
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
