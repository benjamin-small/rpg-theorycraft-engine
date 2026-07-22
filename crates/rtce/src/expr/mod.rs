//! The expression language: pipeline stages, event chances, and scenario
//! weights are written in it; `compile` turns source into a flat postfix
//! `Program` evaluated over a slot array. Fail-closed: unknown identifiers
//! and syntax errors carry a byte position and never guess.

mod compiler;
mod lexer;
mod parser;
pub use compiler::{compile, Op, Program, Symbols};

/// Position-carrying error for every stage (lex/parse/compile).
#[derive(Debug, Clone, PartialEq)]
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
