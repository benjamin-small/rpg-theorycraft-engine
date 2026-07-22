//! Recursive-descent parser. Precedence, loosest to tightest:
//! comparison (`pred`, at most ONE per level — chaining is a positioned
//! error) > + - > * / > unary minus. Functions are a closed set
//! (min/max/clamp/floor/and/or/not) with arity checked at parse time — an
//! unknown function name is an error, never a guess.

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
    Gt,
    Lt,
    Ge,
    Le,
    Eq,
    Ne,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Func {
    Min,
    Max,
    Clamp,
    Floor,
    And,
    Or,
    Not,
}

impl Func {
    pub fn arity(self) -> usize {
        match self {
            Func::Min | Func::Max | Func::And | Func::Or => 2,
            Func::Clamp => 3,
            Func::Floor | Func::Not => 1,
        }
    }
}

pub fn parse(src: &str) -> Result<Ast, ExprError> {
    let toks = tokenize(src)?;
    let mut p = Parser {
        toks,
        pos: 0,
        src_len: src.len(),
    };
    let ast = p.pred()?;
    if p.pos != p.toks.len() {
        return Err(ExprError {
            pos: p.peek_pos(),
            msg: "trailing input".into(),
        });
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
        self.toks
            .get(self.pos)
            .map(|(p, _)| *p)
            .unwrap_or(self.src_len)
    }

    /// `cmpop` if the next token is one of `> < >= <= == !=`.
    fn peek_cmpop(&self) -> Option<BinOp> {
        match self.peek() {
            Some(Tok::Gt) => Some(BinOp::Gt),
            Some(Tok::Lt) => Some(BinOp::Lt),
            Some(Tok::Ge) => Some(BinOp::Ge),
            Some(Tok::Le) => Some(BinOp::Le),
            Some(Tok::EqEq) => Some(BinOp::Eq),
            Some(Tok::Ne) => Some(BinOp::Ne),
            _ => None,
        }
    }

    /// `pred := add (cmpop add)?` — at most ONE comparison per level. A
    /// second cmpop immediately following the first comparison (e.g.
    /// `1 < 2 < 3`) is a positioned "chained comparison" error rather than
    /// silently associating left-to-right, since `(1 < 2) < 3` would
    /// otherwise compare a 0/1 result against `3` without the author
    /// meaning to.
    fn pred(&mut self) -> Result<Ast, ExprError> {
        let lhs = self.expr()?;
        let Some(op) = self.peek_cmpop() else {
            return Ok(lhs);
        };
        self.pos += 1;
        let rhs = self.expr()?;
        let node = Ast::Bin(op, Box::new(lhs), Box::new(rhs));
        if let Some(second_pos) = self.peek_cmpop().map(|_| self.peek_pos()) {
            return Err(ExprError {
                pos: second_pos,
                msg: "chained comparison (e.g. `1 < 2 < 3`) is not allowed — \
                      use `and(1 < 2, 2 < 3)`"
                    .into(),
            });
        }
        Ok(node)
    }

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
                        "and" => Func::And,
                        "or" => Func::Or,
                        "not" => Func::Not,
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
                            // Arguments may themselves be comparisons/booleans
                            // (`and(a >= 40, not(b))`), so parse at `pred`.
                            args.push(self.pred()?);
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
                let inner = self.pred()?;
                if !matches!(self.peek(), Some(Tok::RParen)) {
                    return Err(ExprError {
                        pos: self.peek_pos(),
                        msg: "expected `)`".into(),
                    });
                }
                self.pos += 1;
                Ok(inner)
            }
            _ => Err(ExprError {
                pos,
                msg: "expected number, identifier, or `(`".into(),
            }),
        }
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
                Box::new(Ast::Bin(
                    BinOp::Mul,
                    Box::new(Ast::Num(2.0)),
                    Box::new(Ast::Num(3.0))
                ))
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
                Box::new(Ast::Bin(
                    BinOp::Add,
                    Box::new(Ast::Num(1.0)),
                    Box::new(Ast::Num(2.0))
                )),
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
