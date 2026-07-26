//! The expression language. A `GameDef`'s pipeline stages and event
//! chance/factor formulas are written in it, as are the `SimDef`'s
//! cast times, proc chances, rotation `when` predicates and
//! [`crate::simdef::NumOrExpr`] fields. A `Scenario` is NOT: its phase
//! weights, stat overrides and uptimes are plain numbers, never compiled.
//! `compile` turns source into a flat postfix
//! `Program` evaluated over a slot array. Fail-closed: unknown identifiers
//! and syntax errors carry a byte position and never guess.
//!
//! ## Predicates
//!
//! The grammar adds comparisons (`> < >= <= == !=`) and the boolean
//! functions `and(a, b)` / `or(a, b)` / `not(a)`. Two rules govern how
//! truth values move through the language:
//!
//! - **Truthiness on the way in** is nonzero: any nonzero operand to
//!   `and`/`or`/`not` counts as true, `0.0` as false (no separate bool
//!   type — everything stays `f64`).
//! - **Normalization on the way out** is strict: every comparison and
//!   every boolean function returns EXACTLY `0.0` or `1.0`, never some
//!   other nonzero truthy value, so results compose predictably
//!   (`and(a >= 40, not(b))` chains cleanly).
//!
//! Comparisons sit at their own precedence level, looser than `+ - * /`
//! and unary minus (so `a + 1 > b * 2` parses as `(a + 1) > (b * 2)`), and
//! allow AT MOST ONE comparison per level — `1 < 2 < 3` is a positioned
//! "chained comparison" error rather than silently meaning `(1 < 2) < 3`;
//! write `and(1 < 2, 2 < 3)` instead.

mod compiler;
mod lexer;
mod parser;
pub use compiler::{compile, Op, Program, Symbols};

/// Position-carrying error for every stage (lex/parse/compile).
///
/// `#[non_exhaustive]`: the engine's to extend with more positional or
/// contextual detail; no consumer constructs one.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ExprError {
    /// Byte offset into the source string.
    pub pos: usize,
    /// Human-readable description of what went wrong.
    pub msg: String,
}

impl std::fmt::Display for ExprError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "at byte {}: {}", self.pos, self.msg)
    }
}

impl std::error::Error for ExprError {}
