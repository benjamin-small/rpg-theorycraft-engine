# P1 — Workspace Scaffold + Expression Compiler Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the `rtce` workspace and build the expression language end-to-end (tokenize → parse → compile → evaluate), finishing with a golden fixture that reproduces diablo4-calc's hand-worked `base_hit` 8,573.0184 through the new engine.

**Architecture:** Two crates (`rtce` engine, `rtce-testkit` dev-side harness). The expression module compiles source strings against a caller-supplied symbol table into a flat postfix `Program` (Vec<Op>) evaluated over a `&[f64]` slot array with a fixed-size stack — zero allocation on the hot path. Spec: `docs/superpowers/specs/2026-07-21-rtce-design.md`.

**Tech Stack:** Rust 2021, std-only for `rtce` in P1; `serde_json` only in `rtce-testkit`. TDD red-first; every task commits.

---

### Task 1: Workspace scaffold

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/rtce/Cargo.toml`, `crates/rtce/src/lib.rs`
- Create: `crates/rtce-testkit/Cargo.toml`, `crates/rtce-testkit/src/lib.rs`
- Create: `.gitignore`, `README.md`, `CLAUDE.md`

- [ ] **Step 1: Write the workspace files**

`Cargo.toml`:
```toml
[workspace]
members = ["crates/rtce", "crates/rtce-testkit"]
resolver = "2"

[profile.release]
opt-level = 3
lto = true
```

`crates/rtce/Cargo.toml`:
```toml
[package]
name = "rtce"
version = "0.1.0"
edition = "2021"
description = "RPG theorycraft engine: game algorithms as configuration, compiled once into fast evaluation plans."

[dependencies]

[dev-dependencies]
rtce-testkit = { path = "../rtce-testkit" }
serde_json = "1"
```

`crates/rtce/src/lib.rs`:
```rust
//! rtce — RPG theorycraft engine.
//!
//! The game's ALGORITHM (stats, fold rules, events, pipeline) is
//! configuration, compiled once into a flat evaluation plan; candidates
//! (BuildStates) evaluate in microseconds so external drivers can price
//! tens of thousands of permutations. Design:
//! docs/superpowers/specs/2026-07-21-rtce-design.md.

pub mod expr;
```

`crates/rtce-testkit/Cargo.toml`:
```toml
[package]
name = "rtce-testkit"
version = "0.1.0"
edition = "2021"
description = "Testing harness for rtce consumers: golden-fixture runner and assertion helpers."

[dependencies]
serde_json = "1"
```

`crates/rtce-testkit/src/lib.rs`:
```rust
//! Golden-fixture harness for rtce and its consumer games.
```

`.gitignore`:
```
/target/
```

`README.md`:
```markdown
# rpg-theorycraft-engine (rtce)

A generic, config-driven theorycrafting engine. The game's algorithm —
stats, fold rules, probabilistic events, damage pipeline — is
configuration, compiled once into a fast evaluation plan. Extracted from
the proven patterns of `diablo4-calc` and `poe2-calcs`.

- Design: `docs/superpowers/specs/2026-07-21-rtce-design.md`
- Test: `cargo test --workspace`

Crates: `rtce` (engine), `rtce-testkit` (fixture harness, dev-dependency).
```

`CLAUDE.md`:
```markdown
# rpg-theorycraft-engine

Generic config-driven theorycrafting engine (crates `rtce`, `rtce-testkit`).
See `docs/superpowers/specs/2026-07-21-rtce-design.md` (Done-since = ground
truth) and `docs/superpowers/plans/` for the active plan.

## Commands

    cargo test --workspace     # the whole gate — must be green to commit

## Conventions (inherited from ../diablo4-calc — non-negotiable)

- Small verified slices; every commit carries a hand-checked number where
  one exists (P1 handshake: base_hit 8,573.0184).
- TDD red-first: stub → watch the test fail for the right reason →
  implement → green. Pinned numbers get mutation-checked instead.
- Zero allocation on evaluation hot paths; compilation may be expensive.
- Consumers: diablo4-calc first (its parity suite gates migration);
  knowledge-graph drivers are OUT OF SCOPE — we only price candidates.
```

Note: `crates/rtce/src/expr.rs` does not exist yet — create it as an empty
placeholder module file containing only `//! Expression language.` so the
workspace compiles, or defer `pub mod expr;` to Task 2. Prefer: create
`crates/rtce/src/expr/mod.rs` now with just the doc comment.

- [ ] **Step 2: Verify the workspace builds and tests run (trivially green)**

Run: `cargo test --workspace`
Expected: compiles; `0 passed` in both crates, exit code 0.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "P1: workspace scaffold — rtce + rtce-testkit crates, conventions doc"
```

---

### Task 2: Lexer

**Files:**
- Create: `crates/rtce/src/expr/mod.rs` (module wiring + `ExprError`)
- Create: `crates/rtce/src/expr/lexer.rs`

- [ ] **Step 1: Write module wiring and the failing lexer test**

`crates/rtce/src/expr/mod.rs`:
```rust
//! The expression language: pipeline stages, event chances, and scenario
//! weights are written in it; `compile` turns source into a flat postfix
//! `Program` evaluated over a slot array. Fail-closed: unknown identifiers
//! and syntax errors carry a byte position and never guess.

mod lexer;
mod parser;
mod compiler;

pub use compiler::{compile, Op, Program, Symbols};

/// Position-carrying error for every stage (lex/parse/compile).
#[derive(Debug, Clone, PartialEq)]
pub struct ExprError {
    /// Byte offset into the source string.
    pub pos: usize,
    pub msg: String,
}

impl std::fmt::Display for ExprError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "at byte {}: {}", self.pos, self.msg)
    }
}
```

(`parser` and `compiler` don't exist yet — comment those two lines and the
`pub use` out for this task; Tasks 3–4 uncomment them.)

Append to `crates/rtce/src/expr/lexer.rs` (tests included from the start):
```rust
//! Tokenizer: numbers, identifiers (snake_case dotted names allowed),
//! + - * / ( ) , — everything else is a positioned error.

use super::ExprError;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Num(f64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
    Comma,
}

/// Tokenize `src` into (byte position, token) pairs.
pub fn tokenize(src: &str) -> Result<Vec<(usize, Tok)>, ExprError> {
    todo!("Task 2 Step 3")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        tokenize(src).unwrap().into_iter().map(|(_, t)| t).collect()
    }

    #[test]
    fn arithmetic_identifiers_and_calls_tokenize() {
        assert_eq!(
            toks("1.5 + weapon_avg*(coeff - 2)"),
            vec![
                Tok::Num(1.5),
                Tok::Plus,
                Tok::Ident("weapon_avg".into()),
                Tok::Star,
                Tok::LParen,
                Tok::Ident("coeff".into()),
                Tok::Minus,
                Tok::Num(2.0),
                Tok::RParen,
            ]
        );
        assert_eq!(
            toks("min(a, b)"),
            vec![
                Tok::Ident("min".into()),
                Tok::LParen,
                Tok::Ident("a".into()),
                Tok::Comma,
                Tok::Ident("b".into()),
                Tok::RParen,
            ]
        );
        // Positions are byte offsets.
        let with_pos = tokenize("a + b").unwrap();
        assert_eq!(with_pos[1].0, 2);
        assert_eq!(with_pos[2].0, 4);
    }

    #[test]
    fn bad_characters_error_with_position() {
        let e = tokenize("1 + $x").unwrap_err();
        assert_eq!(e.pos, 4);
        assert!(e.msg.contains('$'), "got: {}", e.msg);
        let e = tokenize("1..5").unwrap_err();
        assert!(e.msg.contains("number"), "got: {}", e.msg);
    }
}
```

- [ ] **Step 2: Run the test, verify it fails on the todo!**

Run: `cargo test -p rtce expr::lexer`
Expected: FAIL (panic "not yet implemented").

- [ ] **Step 3: Implement tokenize**

Replace the `todo!` body:
```rust
pub fn tokenize(src: &str) -> Result<Vec<(usize, Tok)>, ExprError> {
    let b: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || c == '.' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == '.') {
                i += 1;
            }
            let text: String = b[start..i].iter().collect();
            let n = text.parse::<f64>().map_err(|_| ExprError {
                pos: start,
                msg: format!("invalid number `{text}`"),
            })?;
            out.push((start, Tok::Num(n)));
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == '_' || b[i] == '.') {
                i += 1;
            }
            out.push((start, Tok::Ident(b[start..i].iter().collect())));
        } else {
            let tok = match c {
                '+' => Tok::Plus,
                '-' => Tok::Minus,
                '*' => Tok::Star,
                '/' => Tok::Slash,
                '(' => Tok::LParen,
                ')' => Tok::RParen,
                ',' => Tok::Comma,
                other => {
                    return Err(ExprError {
                        pos: i,
                        msg: format!("unexpected character `{other}`"),
                    })
                }
            };
            out.push((i, tok));
            i += 1;
        }
    }
    Ok(out)
}
```

- [ ] **Step 4: Run tests, verify green**

Run: `cargo test -p rtce expr::lexer`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "P1: expression lexer — positioned tokens, fail-closed on bad chars"
```

---

### Task 3: Parser → AST

**Files:**
- Create: `crates/rtce/src/expr/parser.rs`
- Modify: `crates/rtce/src/expr/mod.rs` (uncomment `mod parser;`)

- [ ] **Step 1: Write the failing parser tests + stub**

`crates/rtce/src/expr/parser.rs`:
```rust
//! Recursive-descent parser. Precedence: unary minus > * / > + -.
//! Functions are a closed set (min/max/clamp/floor) with arity checked at
//! parse time — an unknown function name is an error, never a guess.

use super::lexer::{tokenize, Tok};
use super::ExprError;

#[derive(Debug, Clone, PartialEq)]
pub enum Ast {
    Num(f64),
    /// Identifier reference + its byte position (for compile-time errors).
    Ref(String, usize),
    Neg(Box<Ast>),
    Bin(BinOp, Box<Ast>, Box<Ast>),
    Call(Func, Vec<Ast>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Func {
    Min,
    Max,
    Clamp,
    Floor,
}

impl Func {
    pub fn arity(self) -> usize {
        match self {
            Func::Min | Func::Max => 2,
            Func::Clamp => 3,
            Func::Floor => 1,
        }
    }
}

pub fn parse(src: &str) -> Result<Ast, ExprError> {
    let toks = tokenize(src)?;
    let mut p = Parser { toks, pos: 0, src_len: src.len() };
    let ast = p.expr()?;
    if p.pos != p.toks.len() {
        return Err(ExprError { pos: p.peek_pos(), msg: "trailing input".into() });
    }
    Ok(ast)
}

struct Parser {
    toks: Vec<(usize, Tok)>,
    pos: usize,
    src_len: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|(_, t)| t)
    }
    fn peek_pos(&self) -> usize {
        self.toks.get(self.pos).map(|(p, _)| *p).unwrap_or(self.src_len)
    }
    fn expr(&mut self) -> Result<Ast, ExprError> {
        todo!("Task 3 Step 3")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_and_unary() {
        // 1 + 2*3 parses as 1 + (2*3), not (1+2)*3.
        assert_eq!(
            parse("1 + 2*3").unwrap(),
            Ast::Bin(
                BinOp::Add,
                Box::new(Ast::Num(1.0)),
                Box::new(Ast::Bin(BinOp::Mul, Box::new(Ast::Num(2.0)), Box::new(Ast::Num(3.0))))
            )
        );
        // -a * b parses as (-a) * b.
        assert_eq!(
            parse("-a * b").unwrap(),
            Ast::Bin(
                BinOp::Mul,
                Box::new(Ast::Neg(Box::new(Ast::Ref("a".into(), 1)))),
                Box::new(Ast::Ref("b".into(), 5))
            )
        );
        // Parens override.
        assert_eq!(
            parse("(1 + 2) * 3").unwrap(),
            Ast::Bin(
                BinOp::Mul,
                Box::new(Ast::Bin(BinOp::Add, Box::new(Ast::Num(1.0)), Box::new(Ast::Num(2.0)))),
                Box::new(Ast::Num(3.0))
            )
        );
    }

    #[test]
    fn calls_parse_with_arity_checked() {
        assert_eq!(
            parse("clamp(x, 0, 100)").unwrap(),
            Ast::Call(
                Func::Clamp,
                vec![Ast::Ref("x".into(), 6), Ast::Num(0.0), Ast::Num(100.0)]
            )
        );
        let e = parse("min(a)").unwrap_err();
        assert!(e.msg.contains("expects 2"), "got: {}", e.msg);
        let e = parse("shazam(1)").unwrap_err();
        assert!(e.msg.contains("unknown function"), "got: {}", e.msg);
    }

    #[test]
    fn syntax_errors_carry_position() {
        let e = parse("1 + ").unwrap_err();
        assert_eq!(e.pos, 4);
        let e = parse("(1 + 2").unwrap_err();
        assert!(e.msg.contains(')'), "got: {}", e.msg);
        assert!(parse("1 2").unwrap_err().msg.contains("trailing"));
    }
}
```

Uncomment `mod parser;` in `mod.rs`.

- [ ] **Step 2: Run, verify RED on the todo!**

Run: `cargo test -p rtce expr::parser`
Expected: FAIL (not yet implemented).

- [ ] **Step 3: Implement the parser**

Replace `fn expr` and add the remaining methods inside `impl Parser`:
```rust
    fn expr(&mut self) -> Result<Ast, ExprError> {
        let mut lhs = self.term()?;
        while let Some(op) = match self.peek() {
            Some(Tok::Plus) => Some(BinOp::Add),
            Some(Tok::Minus) => Some(BinOp::Sub),
            _ => None,
        } {
            self.pos += 1;
            let rhs = self.term()?;
            lhs = Ast::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn term(&mut self) -> Result<Ast, ExprError> {
        let mut lhs = self.unary()?;
        while let Some(op) = match self.peek() {
            Some(Tok::Star) => Some(BinOp::Mul),
            Some(Tok::Slash) => Some(BinOp::Div),
            _ => None,
        } {
            self.pos += 1;
            let rhs = self.unary()?;
            lhs = Ast::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Ast, ExprError> {
        if matches!(self.peek(), Some(Tok::Minus)) {
            self.pos += 1;
            return Ok(Ast::Neg(Box::new(self.unary()?)));
        }
        self.atom()
    }

    fn atom(&mut self) -> Result<Ast, ExprError> {
        let pos = self.peek_pos();
        match self.toks.get(self.pos).cloned() {
            Some((_, Tok::Num(n))) => {
                self.pos += 1;
                Ok(Ast::Num(n))
            }
            Some((p, Tok::Ident(name))) => {
                self.pos += 1;
                if matches!(self.peek(), Some(Tok::LParen)) {
                    let func = match name.as_str() {
                        "min" => Func::Min,
                        "max" => Func::Max,
                        "clamp" => Func::Clamp,
                        "floor" => Func::Floor,
                        other => {
                            return Err(ExprError {
                                pos: p,
                                msg: format!("unknown function `{other}`"),
                            })
                        }
                    };
                    self.pos += 1; // consume '('
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Some(Tok::RParen)) {
                        loop {
                            args.push(self.expr()?);
                            match self.peek() {
                                Some(Tok::Comma) => self.pos += 1,
                                _ => break,
                            }
                        }
                    }
                    if !matches!(self.peek(), Some(Tok::RParen)) {
                        return Err(ExprError {
                            pos: self.peek_pos(),
                            msg: "expected `)`".into(),
                        });
                    }
                    self.pos += 1;
                    if args.len() != func.arity() {
                        return Err(ExprError {
                            pos: p,
                            msg: format!(
                                "`{name}` expects {} argument(s), got {}",
                                func.arity(),
                                args.len()
                            ),
                        });
                    }
                    Ok(Ast::Call(func, args))
                } else {
                    Ok(Ast::Ref(name, p))
                }
            }
            Some((_, Tok::LParen)) => {
                self.pos += 1;
                let inner = self.expr()?;
                if !matches!(self.peek(), Some(Tok::RParen)) {
                    return Err(ExprError { pos: self.peek_pos(), msg: "expected `)`".into() });
                }
                self.pos += 1;
                Ok(inner)
            }
            _ => Err(ExprError { pos, msg: "expected number, identifier, or `(`".into() }),
        }
    }
```

- [ ] **Step 4: Run, verify green**

Run: `cargo test -p rtce expr::parser`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "P1: expression parser — precedence, unary minus, closed function set with arity"
```

---

### Task 4: Compiler → Program (postfix IR) + evaluator

**Files:**
- Create: `crates/rtce/src/expr/compiler.rs`
- Modify: `crates/rtce/src/expr/mod.rs` (uncomment `mod compiler;` + `pub use`)

- [ ] **Step 1: Write failing compile/eval tests + stubs**

`crates/rtce/src/expr/compiler.rs`:
```rust
//! AST → flat postfix `Program`. Identifiers resolve against a caller
//! `Symbols` table at COMPILE time (unknown name = positioned error);
//! evaluation is a stack machine over `&[f64]` with a fixed-size stack —
//! zero allocation, the search hot path.

use super::parser::{parse, Ast, BinOp, Func};
use super::ExprError;

/// Name → slot resolution, supplied by the caller (the stat registry later).
pub trait Symbols {
    fn slot(&self, name: &str) -> Option<u16>;
}

impl Symbols for std::collections::BTreeMap<String, u16> {
    fn slot(&self, name: &str) -> Option<u16> {
        self.get(name).copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    Const(f64),
    Load(u16),
    Add,
    Sub,
    Mul,
    Div,
    Neg,
    Min,
    Max,
    Clamp,
    Floor,
}

/// Maximum evaluation stack depth; checked at compile, never at eval.
pub const MAX_STACK: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    ops: Vec<Op>,
    /// Peak stack depth (computed during emission, ≤ MAX_STACK).
    pub max_depth: usize,
}

/// Tokenize + parse + emit in one call.
pub fn compile(src: &str, syms: &dyn Symbols) -> Result<Program, ExprError> {
    let ast = parse(src)?;
    let mut ops = Vec::new();
    emit(&ast, syms, &mut ops)?;
    let max_depth = simulate_depth(&ops)?;
    Ok(Program { ops, max_depth })
}

fn emit(ast: &Ast, syms: &dyn Symbols, out: &mut Vec<Op>) -> Result<(), ExprError> {
    todo!("Task 4 Step 3")
}

fn simulate_depth(ops: &[Op]) -> Result<usize, ExprError> {
    todo!("Task 4 Step 3")
}

impl Program {
    /// Evaluate against the slot array. IEEE semantics throughout
    /// (division by zero yields ±inf/NaN — pipelines guard via clamp).
    pub fn eval(&self, slots: &[f64]) -> f64 {
        todo!("Task 4 Step 3")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn syms(names: &[&str]) -> BTreeMap<String, u16> {
        names.iter().enumerate().map(|(i, n)| (n.to_string(), i as u16)).collect()
    }

    #[test]
    fn compiles_to_postfix_and_evaluates() {
        let s = syms(&["a", "b"]);
        let p = compile("1 + a*b", &s).unwrap();
        // Postfix: 1 a b * +
        assert_eq!(
            p,
            Program {
                ops: vec![Op::Const(1.0), Op::Load(0), Op::Load(1), Op::Mul, Op::Add],
                max_depth: 3
            }
        );
        assert_eq!(p.eval(&[2.0, 3.0]), 7.0);
    }

    #[test]
    fn functions_and_unary_evaluate() {
        let s = syms(&["x"]);
        assert_eq!(compile("clamp(x, 0, 10)", &s).unwrap().eval(&[42.0]), 10.0);
        assert_eq!(compile("clamp(x, 0, 10)", &s).unwrap().eval(&[-3.0]), 0.0);
        assert_eq!(compile("min(x, 4) + max(x, 4)", &s).unwrap().eval(&[9.0]), 13.0);
        assert_eq!(compile("floor(x / 4)", &s).unwrap().eval(&[9.0]), 2.0);
        assert_eq!(compile("-x * 2", &s).unwrap().eval(&[5.0]), -10.0);
    }

    #[test]
    fn hand_worked_d4_base_hit_shape() {
        // The diablo4-calc handshake expression (values from its parity
        // suite): 1728 × (314.5/100) × (1 + 462/800)
        //        = 1728 × 3.145 × 1.5775 = 8573.0184 exactly.
        let s = syms(&["weapon_avg", "coeff", "mainstat"]);
        let p = compile("weapon_avg * coeff / 100 * (1 + mainstat / 800)", &s).unwrap();
        assert!((p.eval(&[1728.0, 314.5, 462.0]) - 8573.0184).abs() < 1e-9);
    }

    #[test]
    fn unknown_identifier_is_a_positioned_compile_error() {
        let e = compile("a + mystery", &syms(&["a"])).unwrap_err();
        assert_eq!(e.pos, 4);
        assert!(e.msg.contains("mystery"), "got: {}", e.msg);
    }

    #[test]
    fn depth_guard_rejects_pathological_nesting() {
        // Left-associative chains stay shallow; RIGHT-nested groups push one
        // stack slot per level — 70 levels must trip the MAX_STACK=64 guard.
        let mut src = String::new();
        for _ in 0..70 {
            src.push_str("(1+");
        }
        src.push('1');
        for _ in 0..70 {
            src.push(')');
        }
        let e = compile(&src, &syms(&[])).unwrap_err();
        assert!(e.msg.contains("deep"), "got: {}", e.msg);
    }
}
```

Uncomment `mod compiler;` and the `pub use` line in `mod.rs`.

- [ ] **Step 2: Run, verify RED on the todos**

Run: `cargo test -p rtce expr::compiler`
Expected: FAIL (not yet implemented).

- [ ] **Step 3: Implement emit, simulate_depth, eval**

```rust
fn emit(ast: &Ast, syms: &dyn Symbols, out: &mut Vec<Op>) -> Result<(), ExprError> {
    match ast {
        Ast::Num(n) => out.push(Op::Const(*n)),
        Ast::Ref(name, pos) => match syms.slot(name) {
            Some(slot) => out.push(Op::Load(slot)),
            None => {
                return Err(ExprError {
                    pos: *pos,
                    msg: format!("unknown identifier `{name}`"),
                })
            }
        },
        Ast::Neg(inner) => {
            emit(inner, syms, out)?;
            out.push(Op::Neg);
        }
        Ast::Bin(op, l, r) => {
            emit(l, syms, out)?;
            emit(r, syms, out)?;
            out.push(match op {
                BinOp::Add => Op::Add,
                BinOp::Sub => Op::Sub,
                BinOp::Mul => Op::Mul,
                BinOp::Div => Op::Div,
            });
        }
        Ast::Call(func, args) => {
            for a in args {
                emit(a, syms, out)?;
            }
            out.push(match func {
                Func::Min => Op::Min,
                Func::Max => Op::Max,
                Func::Clamp => Op::Clamp,
                Func::Floor => Op::Floor,
            });
        }
    }
    Ok(())
}

fn simulate_depth(ops: &[Op]) -> Result<usize, ExprError> {
    let mut depth = 0usize;
    let mut peak = 0usize;
    for op in ops {
        let (pops, pushes) = match op {
            Op::Const(_) | Op::Load(_) => (0, 1),
            Op::Neg | Op::Floor => (1, 1),
            Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Min | Op::Max => (2, 1),
            Op::Clamp => (3, 1),
        };
        depth -= pops;
        depth += pushes;
        peak = peak.max(depth);
        if peak > MAX_STACK {
            return Err(ExprError {
                pos: 0,
                msg: format!("expression too deep (stack > {MAX_STACK})"),
            });
        }
    }
    Ok(peak)
}

impl Program {
    pub fn eval(&self, slots: &[f64]) -> f64 {
        let mut stack = [0.0f64; MAX_STACK];
        let mut sp = 0usize;
        for op in &self.ops {
            match op {
                Op::Const(n) => {
                    stack[sp] = *n;
                    sp += 1;
                }
                Op::Load(slot) => {
                    stack[sp] = slots[*slot as usize];
                    sp += 1;
                }
                Op::Neg => stack[sp - 1] = -stack[sp - 1],
                Op::Floor => stack[sp - 1] = stack[sp - 1].floor(),
                Op::Add => {
                    sp -= 1;
                    stack[sp - 1] += stack[sp];
                }
                Op::Sub => {
                    sp -= 1;
                    stack[sp - 1] -= stack[sp];
                }
                Op::Mul => {
                    sp -= 1;
                    stack[sp - 1] *= stack[sp];
                }
                Op::Div => {
                    sp -= 1;
                    stack[sp - 1] /= stack[sp];
                }
                Op::Min => {
                    sp -= 1;
                    stack[sp - 1] = stack[sp - 1].min(stack[sp]);
                }
                Op::Max => {
                    sp -= 1;
                    stack[sp - 1] = stack[sp - 1].max(stack[sp]);
                }
                Op::Clamp => {
                    sp -= 2;
                    let (lo, hi) = (stack[sp], stack[sp + 1]);
                    stack[sp - 1] = stack[sp - 1].clamp(lo, hi);
                }
            }
        }
        stack[sp - 1]
    }
}
```

Note on `simulate_depth` arithmetic: `depth -= pops` before `+= pushes` is
safe because emission order guarantees operands precede operators — a
malformed op sequence cannot come out of `emit`. Keep the subtraction as
`depth = depth.checked_sub(pops).expect("malformed program")` if you prefer
loudness over UB-freedom; either satisfies the tests.

- [ ] **Step 4: Run, verify green (all five tests)**

Run: `cargo test -p rtce expr::compiler`
Expected: 5 passed — including `hand_worked_d4_base_hit_shape` (8573.0184)
and the depth guard.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "P1: compiler + stack evaluator — postfix IR, compile-time symbols/depth, base_hit 8573.0184 by hand"
```

---

### Task 5: rtce-testkit — fixture harness

**Files:**
- Modify: `crates/rtce-testkit/src/lib.rs`

- [ ] **Step 1: Write failing tests + stubs**

`crates/rtce-testkit/src/lib.rs` (replace entirely):
```rust
//! Golden-fixture harness for rtce and its consumer games.
//!
//! House rules (inherited from diablo4-calc M1): a fixture directory that
//! yields ZERO fixtures is a test failure — an empty glob must never pass
//! silently; every fixture carries `name` and `source` provenance.

use std::path::Path;

/// Relative-tolerance assertion with a context message.
pub fn assert_close(actual: f64, expected: f64, rel_tol: f64, ctx: &str) {
    let denom = expected.abs().max(1e-12);
    let rel = (actual - expected).abs() / denom;
    assert!(
        rel <= rel_tol,
        "{ctx}: {actual} != {expected} (rel err {rel:.3e} > {rel_tol:.1e})"
    );
}

/// Invoke `f(name, json)` for every `*.json` fixture in `dir`, sorted by
/// file name. PANICS if the directory holds no fixtures (or is missing) —
/// the empty-glob rule.
pub fn for_each_fixture(dir: &Path, mut f: impl FnMut(&str, &serde_json::Value)) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("fixture dir {}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    entries.sort();
    assert!(
        !entries.is_empty(),
        "no fixtures in {} — an empty suite must not pass",
        dir.display()
    );
    for path in entries {
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let v: serde_json::Value = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{}: invalid JSON: {e}", path.display()));
        assert!(
            v.get("name").is_some() && v.get("source").is_some(),
            "{}: fixtures must carry `name` and `source` provenance",
            path.display()
        );
        f(path.file_stem().unwrap().to_str().unwrap(), &v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn assert_close_accepts_within_and_rejects_outside_tolerance() {
        assert_close(100.0005, 100.0, 1e-5, "ok");
        let r = std::panic::catch_unwind(|| assert_close(101.0, 100.0, 1e-5, "off"));
        assert!(r.is_err(), "1% off must fail a 1e-5 tolerance");
    }

    #[test]
    fn empty_fixture_dir_panics() {
        let dir = std::env::temp_dir().join("rtce-testkit-empty");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let r = std::panic::catch_unwind(|| for_each_fixture(&dir, |_, _| {}));
        assert!(r.is_err(), "empty dir must panic");
    }

    #[test]
    fn fixtures_iterate_sorted_and_demand_provenance() {
        let dir = std::env::temp_dir().join("rtce-testkit-two");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("b.json"), r#"{"name":"b","source":"t","v":2}"#).unwrap();
        fs::write(dir.join("a.json"), r#"{"name":"a","source":"t","v":1}"#).unwrap();
        let mut seen = Vec::new();
        for_each_fixture(&dir, |name, v| seen.push((name.to_string(), v["v"].as_i64().unwrap())));
        assert_eq!(seen, vec![("a".into(), 1), ("b".into(), 2)]);

        fs::write(dir.join("c.json"), r#"{"v":3}"#).unwrap();
        let r = std::panic::catch_unwind(|| for_each_fixture(&dir, |_, _| {}));
        assert!(r.is_err(), "missing provenance must panic");
    }
}
```

(The implementations above are complete — the RED step here is running the
tests BEFORE saving the implementations, i.e., stub `assert_close` and
`for_each_fixture` with `todo!()` first, watch the three tests fail, then
paste the bodies. Keep that ordering.)

- [ ] **Step 2: RED run with todo!-stubbed bodies**

Run: `cargo test -p rtce-testkit`
Expected: 3 FAILED (not yet implemented).

- [ ] **Step 3: Paste the real bodies (shown above)**

- [ ] **Step 4: Run, verify green**

Run: `cargo test -p rtce-testkit`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "P1: rtce-testkit — assert_close + fixture iteration, empty-glob and provenance rules"
```

---

### Task 6: End-to-end golden — the cross-repo handshake

**Files:**
- Create: `crates/rtce/tests/fixtures/d4_base_hit.json`
- Create: `crates/rtce/tests/golden.rs`

- [ ] **Step 1: Write the fixture and the runner**

`crates/rtce/tests/fixtures/d4_base_hit.json`:
```json
{
  "name": "d4_base_hit_handshake",
  "source": "hand-worked 2026-07-21, mirrors diablo4-calc parity suite: 1728 × (314.5/100) × (1 + 462/800) = 8573.0184 (fireball_rank9 base_hit)",
  "expr": "weapon_avg * coeff / 100 * (1 + mainstat / 800)",
  "slots": { "weapon_avg": 1728.0, "coeff": 314.5, "mainstat": 462.0 },
  "expect": 8573.0184,
  "rel_tolerance": 1e-9
}
```

`crates/rtce/tests/golden.rs`:
```rust
//! Golden fixtures through the full pipeline: JSON → compile → eval →
//! assert. The first fixture is the cross-repo handshake with
//! diablo4-calc's parity suite (base_hit 8,573.0184).

use rtce::expr::compile;
use rtce_testkit::{assert_close, for_each_fixture};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[test]
fn golden_fixtures_reproduce_pinned_values() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for_each_fixture(&dir, |name, v| {
        let slots_json = v["slots"].as_object().unwrap_or_else(|| panic!("{name}: slots"));
        let mut names: Vec<&String> = slots_json.keys().collect();
        names.sort();
        let syms: BTreeMap<String, u16> =
            names.iter().enumerate().map(|(i, n)| ((*n).clone(), i as u16)).collect();
        let slots: Vec<f64> =
            names.iter().map(|n| slots_json[*n].as_f64().unwrap()).collect();
        let program = compile(v["expr"].as_str().unwrap(), &syms)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let actual = program.eval(&slots);
        assert_close(
            actual,
            v["expect"].as_f64().unwrap(),
            v["rel_tolerance"].as_f64().unwrap_or(1e-9),
            name,
        );
    });
}
```

- [ ] **Step 2: Run, verify green**

Run: `cargo test -p rtce --test golden`
Expected: 1 passed (`d4_base_hit_handshake` inside).

- [ ] **Step 3: Mutation-check the pin**

Edit the fixture's `"expect"` to `8573.02`, run the test, verify it FAILS
with the relative-error message; restore `8573.0184`, verify green again.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "P1 gate: golden handshake — d4 base_hit 8,573.0184 through JSON → compile → eval (mutation-proven)"
```

---

### Task 7: Close out P1

**Files:**
- Modify: `docs/superpowers/specs/2026-07-21-rtce-design.md` (Done-since)

- [ ] **Step 1: Append a Done-since entry to the spec**

Add at the end of the spec:
```markdown

## Done since

- 2026-07-21 — P1 complete: workspace (rtce + rtce-testkit), expression
  language end-to-end (lexer → parser → postfix Program → stack evaluator;
  min/max/clamp/floor; positioned fail-closed errors; compile-time symbol
  resolution and depth guard, zero-alloc eval). Testkit enforces the
  empty-glob and provenance rules. Pinned handshake: d4 base_hit
  8,573.0184 via golden fixture, mutation-proven.
```

- [ ] **Step 2: Full-workspace green check**

Run: `cargo test --workspace`
Expected: all suites pass, zero failures.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "P1 complete: expression engine green end-to-end — handshake 8,573.0184"
```
