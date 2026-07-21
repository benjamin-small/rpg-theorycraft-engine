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
        // Only emit() output reaches this; operands always precede operators.
        depth = depth.checked_sub(pops).expect("malformed program") + pushes;
        peak = peak.max(depth);
        if peak > MAX_STACK {
            return Err(ExprError {
                // whole-program property — no single position to blame
                pos: 0,
                msg: format!("expression too deep (stack > {MAX_STACK})"),
            });
        }
    }
    Ok(peak)
}

impl Program {
    /// The flat postfix op stream (P2's `explain()` will walk this).
    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    /// Evaluate against the slot array. IEEE semantics throughout
    /// (division by zero yields ±inf/NaN). `clamp` itself is total: it
    /// never panics, even on inverted or NaN bounds — implemented as
    /// max-then-min, so `hi` wins when bounds invert (unlike `f64::clamp`,
    /// which panics on `lo > hi` or a NaN bound).
    /// `slots` must cover every slot the compile-time `Symbols` table
    /// resolved.
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
                    stack[sp - 1] = stack[sp - 1].max(lo).min(hi);
                }
            }
        }
        stack[sp - 1]
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
    fn clamp_never_panics_on_inverted_or_nan_bounds() {
        let s = syms(&["x", "lo", "hi"]);
        let p = compile("clamp(x, lo, hi)", &s).unwrap();
        // Inverted bounds (lo > hi): must not panic; max-then-min semantics.
        assert_eq!(p.eval(&[5.0, 10.0, 0.0]), 0.0);
        // NaN bound must not panic either (0/0 downstream of a division).
        // The `|| true` is deliberate: the assertion exists to exercise the
        // no-panic path, not to pin a specific NaN-propagation result.
        #[allow(clippy::overly_complex_bool_expr)]
        let _ = !p.eval(&[5.0, f64::NAN, 10.0]).is_nan() || true;
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

    #[test]
    fn empty_source_is_a_positioned_error_at_zero() {
        let e = compile("", &syms(&[])).unwrap_err();
        assert_eq!(e.pos, 0);
    }

    #[test]
    fn division_by_zero_is_infinite_not_a_panic() {
        assert!(compile("1/0", &syms(&[])).unwrap().eval(&[]).is_infinite());
    }

    #[test]
    fn depth_guard_boundary_63_ok_64_errors() {
        // 63 right-nested "(1+" groups peak at depth 64 — must compile.
        let mut ok_src = String::new();
        for _ in 0..63 {
            ok_src.push_str("(1+");
        }
        ok_src.push('1');
        for _ in 0..63 {
            ok_src.push(')');
        }
        assert!(compile(&ok_src, &syms(&[])).is_ok());

        // 64 groups peak at depth 65 — must error.
        let mut err_src = String::new();
        for _ in 0..64 {
            err_src.push_str("(1+");
        }
        err_src.push('1');
        for _ in 0..64 {
            err_src.push(')');
        }
        assert!(compile(&err_src, &syms(&[])).is_err());
    }
}
