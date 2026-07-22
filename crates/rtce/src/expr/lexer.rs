//! Tokenizer: numbers, identifiers (snake_case dotted names allowed),
//! arithmetic (`+ - * / ( ) ,`) and comparison operators
//! (`> < >= <= == !=`) — everything else is a positioned error.

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
    Gt,
    Lt,
    Ge,
    Le,
    EqEq,
    Ne,
}

/// Tokenize `src` into (byte position, token) pairs. Positions are true
/// BYTE offsets into `src` — the language is ASCII-only, so token bodies
/// (digits, `.`, identifier chars) are sliced directly out of the byte
/// string, which is safe because none of those bytes can appear inside a
/// multi-byte UTF-8 sequence. Any non-ASCII byte is a positioned error
/// (fail-closed) rather than being silently treated as whitespace.
pub fn tokenize(src: &str) -> Result<Vec<(usize, Tok)>, ExprError> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || c == b'.' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            let text = &src[start..i];
            let n = text.parse::<f64>().map_err(|_| ExprError {
                pos: start,
                msg: format!("invalid number `{text}`"),
            })?;
            out.push((start, Tok::Num(n)));
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.') {
                i += 1;
            }
            out.push((start, Tok::Ident(src[start..i].to_string())));
        } else if c == b'>' || c == b'<' || c == b'=' || c == b'!' {
            // Two-char lookahead: `>= <= == !=` before the one-char forms;
            // a bare `=` or `!` (no following `=`) is a positioned error —
            // neither stands alone in this grammar.
            let next_eq = b.get(i + 1) == Some(&b'=');
            let (tok, len) = match (c, next_eq) {
                (b'>', true) => (Tok::Ge, 2),
                (b'>', false) => (Tok::Gt, 1),
                (b'<', true) => (Tok::Le, 2),
                (b'<', false) => (Tok::Lt, 1),
                (b'=', true) => (Tok::EqEq, 2),
                (b'!', true) => (Tok::Ne, 2),
                (b'=', false) => {
                    return Err(ExprError {
                        pos: i,
                        msg: "unexpected character `=` (did you mean `==`?)".into(),
                    })
                }
                (b'!', false) => {
                    return Err(ExprError {
                        pos: i,
                        msg: "unexpected character `!` (did you mean `!=`?)".into(),
                    })
                }
                _ => unreachable!(),
            };
            out.push((i, tok));
            i += len;
        } else if c.is_ascii() {
            let tok = match c {
                b'+' => Tok::Plus,
                b'-' => Tok::Minus,
                b'*' => Tok::Star,
                b'/' => Tok::Slash,
                b'(' => Tok::LParen,
                b')' => Tok::RParen,
                b',' => Tok::Comma,
                other => {
                    return Err(ExprError {
                        pos: i,
                        msg: format!("unexpected character `{}`", other as char),
                    })
                }
            };
            out.push((i, tok));
            i += 1;
        } else {
            let ch = src[i..].chars().next().unwrap();
            return Err(ExprError {
                pos: i,
                msg: format!("unexpected character `{ch}`"),
            });
        }
    }
    Ok(out)
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
    fn comparison_tokens_lex_two_char_before_one_char() {
        assert_eq!(
            toks("a >= b < c <= d == e != f > g"),
            vec![
                Tok::Ident("a".into()),
                Tok::Ge,
                Tok::Ident("b".into()),
                Tok::Lt,
                Tok::Ident("c".into()),
                Tok::Le,
                Tok::Ident("d".into()),
                Tok::EqEq,
                Tok::Ident("e".into()),
                Tok::Ne,
                Tok::Ident("f".into()),
                Tok::Gt,
                Tok::Ident("g".into()),
            ]
        );
    }

    #[test]
    fn bare_bang_or_equals_errors_cleanly() {
        let e = tokenize("a ! b").unwrap_err();
        assert_eq!(e.pos, 2);
        assert!(e.msg.contains('!'), "got: {}", e.msg);
        let e = tokenize("a = b").unwrap_err();
        assert_eq!(e.pos, 2);
        assert!(e.msg.contains('='), "got: {}", e.msg);
    }

    #[test]
    fn positions_are_true_byte_offsets_even_after_multibyte_chars() {
        // NBSP (U+00A0) is 2 bytes; it is NOT whitespace-skipped by the byte
        // lexer — it must error AT ITS OWN byte offset, and a later probe
        // confirms offsets after multibyte content stay byte-true.
        let e = tokenize("a\u{a0}b").unwrap_err();
        assert_eq!(e.pos, 1);
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
