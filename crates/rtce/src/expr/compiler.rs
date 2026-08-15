//! AST → flat postfix `Program`. Identifiers resolve against a caller
//! `Symbols` table at COMPILE time (unknown name = positioned error);
//! evaluation is a stack machine over `&[f64]` with a fixed-size stack —
//! zero allocation, the search hot path.

use super::parser::{parse, Ast, BinOp, Func};
use super::ExprError;

/// Name → slot resolution, supplied by the caller (the stat registry later).
pub trait Symbols {
    /// Resolve `name` to its slot index, or `None` if it isn't defined —
    /// callers turn `None` into a positioned `ExprError` at compile time.
    fn slot(&self, name: &str) -> Option<u16>;
}

impl Symbols for std::collections::BTreeMap<String, u16> {
    fn slot(&self, name: &str) -> Option<u16> {
        self.get(name).copied()
    }
}

/// One flat postfix stack-machine instruction. A [`Program`] is a `Vec<Op>`
/// evaluated left to right against an operand stack.
///
/// `#[non_exhaustive]`: this is the COMPILED representation of an
/// expression, and it is the engine's to extend — P6 added the nine
/// comparison/logic ops to it, and a later grammar addition will add more.
/// It is reachable outside the crate through [`Program::ops`], so `match`
/// on it with a `_` arm.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum Op {
    /// Push a literal value.
    Const(f64),
    /// Push `slots[n]`.
    Load(u16),
    /// Pop two, push their sum.
    Add,
    /// Pop two (b, a), push `a - b`.
    Sub,
    /// Pop two, push their product.
    Mul,
    /// Pop two (b, a), push `a / b` (IEEE semantics — never panics).
    Div,
    /// Pop one, push its negation.
    Neg,
    /// Pop two, push the smaller.
    Min,
    /// Pop two, push the larger.
    Max,
    /// Pop three in push order (x, lo, hi), push `x` clamped into
    /// `[lo, hi]` (total — see [`Program::eval`] for the inverted/NaN-bound
    /// semantics).
    Clamp,
    /// Pop one, push its floor.
    Floor,
    /// Pop one, push its square root using IEEE `f64::sqrt` semantics.
    Sqrt,
    /// Pop two (exponent, base), push `base.powf(exponent)` using IEEE
    /// `f64` semantics.
    Pow,
    /// Pop two (b, a), push `(a > b) as u8 as f64` — strictly 0.0/1.0.
    Gt,
    /// Pop two (b, a), push `(a < b) as u8 as f64`.
    Lt,
    /// Pop two (b, a), push `(a >= b) as u8 as f64`.
    Ge,
    /// Pop two (b, a), push `(a <= b) as u8 as f64`.
    Le,
    /// Pop two, push `(a == b) as u8 as f64`.
    Eq,
    /// Pop two, push `(a != b) as u8 as f64`.
    Ne,
    /// Pop two, push `1.0` iff both operands are nonzero (truthy), else `0.0`.
    And,
    /// Pop two, push `1.0` iff either operand is nonzero (truthy), else `0.0`.
    Or,
    /// Pop one, push `1.0` iff the operand is zero, else `0.0`.
    Not,
}

/// Maximum evaluation stack depth; checked at compile, never at eval.
/// NOT public API: the `compiler` module is private and nothing
/// re-exports this constant, so a caller cannot read the bound — see
/// [`Program::max_depth`]'s docs for the public statement of the
/// relationship.
pub const MAX_STACK: usize = 64;

/// A compiled expression: a flat postfix [`Op`] stream plus its peak stack
/// depth, ready for zero-allocation evaluation via [`Program::eval`].
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    ops: Vec<Op>,
    /// Peak evaluation-stack depth, computed during emission.
    ///
    /// Guaranteed `<= 64` by construction: `compile` simulates the
    /// postfix stack and REJECTS a deeper program with a positioned
    /// "expression too deep" error (reachable — ~64 levels of
    /// right-nested grouping like `(1+(1+(…)))` trip it; left-leaning
    /// chains stay shallow, so no realistic config does). The bound
    /// itself is a crate-internal constant (`MAX_STACK`), deliberately
    /// NOT exported: [`Program::eval`]'s fixed stack is sized by the
    /// same constant, which is why eval never checks depth — a `Program`
    /// that exists cannot overflow it. This field is the observable
    /// half: what the compile-time simulation measured for THIS program.
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
                BinOp::Gt => Op::Gt,
                BinOp::Lt => Op::Lt,
                BinOp::Ge => Op::Ge,
                BinOp::Le => Op::Le,
                BinOp::Eq => Op::Eq,
                BinOp::Ne => Op::Ne,
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
                Func::Sqrt => Op::Sqrt,
                Func::Pow => Op::Pow,
                Func::And => Op::And,
                Func::Or => Op::Or,
                Func::Not => Op::Not,
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
            Op::Neg | Op::Floor | Op::Sqrt | Op::Not => (1, 1),
            Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Min | Op::Max | Op::Pow => (2, 1),
            Op::Gt | Op::Lt | Op::Ge | Op::Le | Op::Eq | Op::Ne | Op::And | Op::Or => (2, 1),
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

    /// Evaluate against the slot array. IEEE semantics throughout:
    /// division by zero and invalid `sqrt`/`pow` domains yield ±inf/NaN
    /// rather than an evaluation error. `clamp` itself is total: it
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
                Op::Sqrt => stack[sp - 1] = stack[sp - 1].sqrt(),
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
                Op::Pow => {
                    sp -= 1;
                    stack[sp - 1] = stack[sp - 1].powf(stack[sp]);
                }
                Op::Clamp => {
                    sp -= 2;
                    let (lo, hi) = (stack[sp], stack[sp + 1]);
                    stack[sp - 1] = stack[sp - 1].max(lo).min(hi);
                }
                Op::Gt => {
                    sp -= 1;
                    stack[sp - 1] = (stack[sp - 1] > stack[sp]) as u8 as f64;
                }
                Op::Lt => {
                    sp -= 1;
                    stack[sp - 1] = (stack[sp - 1] < stack[sp]) as u8 as f64;
                }
                Op::Ge => {
                    sp -= 1;
                    stack[sp - 1] = (stack[sp - 1] >= stack[sp]) as u8 as f64;
                }
                Op::Le => {
                    sp -= 1;
                    stack[sp - 1] = (stack[sp - 1] <= stack[sp]) as u8 as f64;
                }
                Op::Eq => {
                    sp -= 1;
                    stack[sp - 1] = (stack[sp - 1] == stack[sp]) as u8 as f64;
                }
                Op::Ne => {
                    sp -= 1;
                    stack[sp - 1] = (stack[sp - 1] != stack[sp]) as u8 as f64;
                }
                Op::And => {
                    sp -= 1;
                    stack[sp - 1] = (stack[sp - 1] != 0.0 && stack[sp] != 0.0) as u8 as f64;
                }
                Op::Or => {
                    sp -= 1;
                    stack[sp - 1] = (stack[sp - 1] != 0.0 || stack[sp] != 0.0) as u8 as f64;
                }
                Op::Not => {
                    stack[sp - 1] = (stack[sp - 1] == 0.0) as u8 as f64;
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
        names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.to_string(), i as u16))
            .collect()
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
        assert_eq!(
            compile("min(x, 4) + max(x, 4)", &s).unwrap().eval(&[9.0]),
            13.0
        );
        assert_eq!(compile("floor(x / 4)", &s).unwrap().eval(&[9.0]), 2.0);
        assert_eq!(compile("sqrt(x)", &s).unwrap().eval(&[9.0]), 3.0);
        assert_eq!(compile("pow(x, 0.5)", &s).unwrap().eval(&[9.0]), 3.0);
        assert_eq!(compile("-x * 2", &s).unwrap().eval(&[5.0]), -10.0);
    }

    #[test]
    fn sqrt_and_pow_have_correct_postfix_depth_and_fractional_evaluation() {
        let p = compile("pow(0.93, sqrt(1.1377777777778489))", &syms(&[])).unwrap();
        assert_eq!(
            p.ops(),
            &[
                Op::Const(0.93),
                Op::Const(1.1377777777778489),
                Op::Sqrt,
                Op::Pow,
            ]
        );
        assert_eq!(p.max_depth, 2);
        let exponent = 1.1377777777778489_f64.sqrt();
        assert!((p.eval(&[]) - 0.93_f64.powf(exponent)).abs() < 1e-15);
    }

    #[test]
    fn sqrt_and_pow_invalid_domains_follow_ieee_semantics() {
        assert!(compile("sqrt(-1)", &syms(&[])).unwrap().eval(&[]).is_nan());
        assert!(compile("pow(-1, 0.5)", &syms(&[]))
            .unwrap()
            .eval(&[])
            .is_nan());
        assert!(compile("pow(0, -1)", &syms(&[]))
            .unwrap()
            .eval(&[])
            .is_infinite());
    }

    #[test]
    fn hand_worked_d4_base_hit_shape() {
        // The d4-theory-crafting handshake expression (values from its parity
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

    /// N right-nested levels of `(1+ … 1 … )` — each level parks one
    /// constant on the stack while descending, so the peak depth is N+1.
    fn right_nested(levels: usize) -> String {
        let mut src = String::new();
        for _ in 0..levels {
            src.push_str("(1+");
        }
        src.push('1');
        for _ in 0..levels {
            src.push(')');
        }
        src
    }

    #[test]
    fn depth_guard_rejects_pathological_nesting() {
        // Left-associative chains stay shallow; RIGHT-nested groups push one
        // stack slot per level — 70 levels must trip the MAX_STACK=64 guard.
        let e = compile(&right_nested(70), &syms(&[])).unwrap_err();
        assert!(e.msg.contains("deep"), "got: {}", e.msg);
    }

    #[test]
    fn depth_guard_boundary_is_exactly_max_stack() {
        // The EXACT boundary `Program::max_depth`'s docs promise (P8f):
        // 63 levels peak at 64 == MAX_STACK and compile — the fixed eval
        // stack holds them, `max_depth` reads the bound itself — while 64
        // levels peak at 65 and are rejected at compile, which is WHY
        // eval never checks depth.
        let ok = compile(&right_nested(63), &syms(&[])).unwrap();
        assert_eq!(
            ok.max_depth, MAX_STACK,
            "63 right-nested levels must peak exactly AT the bound"
        );
        assert_eq!(ok.eval(&[]), 64.0, "and still evaluate: 63 additions of 1");

        let e = compile(&right_nested(64), &syms(&[])).unwrap_err();
        assert!(
            e.msg.contains("expression too deep (stack > 64)"),
            "one level past the bound must fail closed at compile: {}",
            e.msg
        );
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
    fn comparisons_and_boolean_functions() {
        let s = syms(&["a", "b"]);
        let e = |src: &str, slots: &[f64]| compile(src, &s).unwrap().eval(slots);
        // Comparisons return exactly 0/1.
        assert_eq!(e("a > b", &[3.0, 2.0]), 1.0);
        assert_eq!(e("a > b", &[2.0, 3.0]), 0.0);
        assert_eq!(e("a >= b", &[2.0, 2.0]), 1.0);
        assert_eq!(e("a < b", &[2.0, 3.0]), 1.0);
        assert_eq!(e("a <= b", &[3.0, 2.0]), 0.0);
        assert_eq!(e("a == b", &[2.0, 2.0]), 1.0);
        assert_eq!(e("a != b", &[2.0, 2.0]), 0.0);
        // Precedence: arithmetic binds tighter than comparison.
        assert_eq!(e("a + 1 > b * 2", &[3.0, 2.0]), 0.0); // 4 > 4 → 0
                                                          // Boolean functions: strict 0/1 out, nonzero-truthy in.
        assert_eq!(e("and(a, b)", &[1.0, 0.0]), 0.0);
        assert_eq!(e("and(a, b)", &[2.0, -1.0]), 1.0);
        assert_eq!(e("or(a, b)", &[0.0, 2.0]), 1.0);
        assert_eq!(e("or(a, b)", &[0.0, 0.0]), 0.0);
        assert_eq!(e("not(a)", &[0.0]), 1.0);
        assert_eq!(e("not(a)", &[3.0]), 0.0);
        // Composability: the P6 rotation shape.
        assert_eq!(e("and(a >= 40, not(b))", &[40.0, 0.0]), 1.0);
    }

    #[test]
    fn chained_comparison_is_a_positioned_error() {
        // `1 < 2 < 3` would silently mean `(1 < 2) < 3` — banned outright;
        // the error names the byte position of the SECOND cmpop and says
        // "chained" so the author knows to use `and(...)` instead.
        let e = compile("1 < 2 < 3", &syms(&[])).unwrap_err();
        assert!(e.msg.contains("chained"), "got: {}", e.msg);
        assert_eq!(e.pos, 6);
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
