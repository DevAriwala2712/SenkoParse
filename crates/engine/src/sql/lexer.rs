use crate::{err, Result};

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String), // lower-cased
    Num(String),
    Str(String),
    Sym(&'static str),
    Eof,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub start: usize,
    pub end: usize,
}

pub fn lex(s: &str) -> Result<Vec<Token>> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        let start = i;
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'-' && b.get(i + 1) == Some(&b'-') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        let tok = if c.is_ascii_alphabetic() || c == b'_' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            Tok::Ident(s[start..i].to_ascii_lowercase())
        } else if c == b'"' || c == b'`' {
            let q = c;
            i += 1;
            let mut name = Vec::new();
            loop {
                match b.get(i) {
                    None => return Err(err!("unterminated quoted identifier")),
                    Some(&x) if x == q => {
                        if b.get(i + 1) == Some(&q) {
                            name.push(q);
                            i += 2;
                        } else {
                            i += 1;
                            break;
                        }
                    }
                    Some(&x) => {
                        name.push(x);
                        i += 1;
                    }
                }
            }
            Tok::Ident(String::from_utf8_lossy(&name).to_ascii_lowercase())
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).map_or(false, |d| d.is_ascii_digit())) {
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                let mut j = i + 1;
                if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
                    j += 1;
                }
                if j < b.len() && b[j].is_ascii_digit() {
                    while j < b.len() && b[j].is_ascii_digit() {
                        j += 1;
                    }
                    i = j;
                }
            }
            Tok::Num(s[start..i].to_string())
        } else if c == b'\'' {
            i += 1;
            let mut v = Vec::new();
            loop {
                match b.get(i) {
                    None => return Err(err!("unterminated string literal")),
                    Some(b'\'') => {
                        if b.get(i + 1) == Some(&b'\'') {
                            v.push(b'\'');
                            i += 2;
                        } else {
                            i += 1;
                            break;
                        }
                    }
                    Some(&x) => {
                        v.push(x);
                        i += 1;
                    }
                }
            }
            Tok::Str(String::from_utf8_lossy(&v).into_owned())
        } else {
            let two = s.get(i..i + 2).unwrap_or("");
            let (sym, n): (&'static str, usize) = match two {
                "<=" => ("<=", 2),
                ">=" => (">=", 2),
                "<>" => ("<>", 2),
                "!=" => ("<>", 2),
                "==" => ("=", 2),
                _ => match c {
                    b'=' => ("=", 1),
                    b'<' => ("<", 1),
                    b'>' => (">", 1),
                    b'+' => ("+", 1),
                    b'-' => ("-", 1),
                    b'*' => ("*", 1),
                    b'/' => ("/", 1),
                    b'(' => ("(", 1),
                    b')' => (")", 1),
                    b',' => (",", 1),
                    b'.' => (".", 1),
                    b';' => (";", 1),
                    _ => return Err(err!("unexpected character '{}' at offset {i}", c as char)),
                },
            };
            i += n;
            Tok::Sym(sym)
        };
        out.push(Token { tok, start, end: i });
    }
    out.push(Token { tok: Tok::Eof, start: s.len(), end: s.len() });
    Ok(out)
}
