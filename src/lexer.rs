use crate::error::{Error, Pos};

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    Int(i64),
    Str(String),
    Sym(&'static str),
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub pos: Pos,
    /// Byte offsets into the source, used to recover the text of contracts and asserts.
    pub start: usize,
    pub end: usize,
    /// True when a newline separates this token from the previous one. Glyph has no
    /// semicolons, so `f\n(x)` must not parse as a call.
    pub nl_before: bool,
}

const SYMS2: [&str; 5] = ["->", "==", "!=", "<=", ">="];
const SYMS1: [&str; 16] = [
    "(", ")", "{", "}", "[", "]", ",", ":", "+", "-", "*", "/", "%", "<", ">", "=",
];

pub fn lex(src: &str) -> Result<Vec<Token>, Error> {
    let cs: Vec<(usize, char)> = src.char_indices().collect();
    let off = |i: usize| if i < cs.len() { cs[i].0 } else { src.len() };
    let mut toks = Vec::new();
    let (mut i, mut line, mut col, mut nl) = (0, 1, 1, false);

    while i < cs.len() {
        let c = cs[i].1;
        let pos = Pos { line, col };
        let start = i;
        if c == '\n' {
            line += 1;
            col = 1;
            nl = true;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            col += 1;
            i += 1;
            continue;
        }
        if c == '#' {
            while i < cs.len() && cs[i].1 != '\n' {
                i += 1;
            }
            continue;
        }

        let tok = if c.is_ascii_digit() {
            while i < cs.len() && (cs[i].1.is_ascii_digit() || cs[i].1 == '_') {
                i += 1;
            }
            let text: String = cs[start..i]
                .iter()
                .map(|p| p.1)
                .filter(|c| *c != '_')
                .collect();
            Tok::Int(text.parse().map_err(|_| {
                Error::new(
                    "syntax",
                    pos,
                    format!("integer literal `{}` is too large", text),
                )
            })?)
        } else if c.is_alphabetic() || c == '_' {
            while i < cs.len() && (cs[i].1.is_alphanumeric() || cs[i].1 == '_') {
                i += 1;
            }
            Tok::Ident(cs[start..i].iter().map(|p| p.1).collect())
        } else if c == '"' {
            i += 1;
            let mut s = String::new();
            loop {
                if i >= cs.len() || cs[i].1 == '\n' {
                    return Err(Error::new("syntax", pos, "unterminated string literal"));
                }
                let ch = cs[i].1;
                i += 1;
                match ch {
                    '"' => break,
                    '\\' => {
                        let e = if i < cs.len() { cs[i].1 } else { ' ' };
                        i += 1;
                        s.push(match e {
                            'n' => '\n',
                            't' => '\t',
                            '"' => '"',
                            '\\' => '\\',
                            _ => {
                                return Err(Error::new(
                                    "syntax",
                                    pos,
                                    format!("unknown escape `\\{}` in string", e),
                                )
                                .hint("supported escapes: \\n \\t \\\" \\\\"))
                            }
                        });
                    }
                    _ => s.push(ch),
                }
            }
            Tok::Str(s)
        } else {
            let rest = &src[off(i)..];
            if let Some(s) = SYMS2.iter().find(|s| rest.starts_with(**s)) {
                i += 2;
                Tok::Sym(s)
            } else if let Some(s) = SYMS1.iter().find(|s| rest.starts_with(**s)) {
                i += 1;
                Tok::Sym(s)
            } else {
                let mut e = Error::new("syntax", pos, format!("unexpected character `{}`", c));
                if c == '!' {
                    e = e.hint("use `not` for negation and `!=` for inequality");
                } else if c == '&' || c == '|' {
                    e = e.hint("use `and` / `or`");
                }
                return Err(e);
            }
        };
        col += i - start;
        toks.push(Token {
            tok,
            pos,
            start: off(start),
            end: off(i),
            nl_before: nl,
        });
        nl = false;
    }
    toks.push(Token {
        tok: Tok::Eof,
        pos: Pos { line, col },
        start: src.len(),
        end: src.len(),
        nl_before: true,
    });
    Ok(toks)
}
