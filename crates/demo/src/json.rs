//! Minimal reader for the flat one-line JSON objects the recordings use (string and number values only).
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub enum V {
    S(String),
    N(f64),
}

pub type Obj = HashMap<String, V>;

pub fn parse_obj(line: &str) -> Option<Obj> {
    let mut it = line.trim().chars().peekable();
    if it.next()? != '{' {
        return None;
    }
    let mut m = Obj::new();
    loop {
        while it.peek().is_some_and(|c| c.is_whitespace() || *c == ',') {
            it.next();
        }
        match it.peek()? {
            '}' => return Some(m),
            '"' => {}
            _ => return None,
        }
        let key = string(&mut it)?;
        while it.peek().is_some_and(|c| c.is_whitespace()) {
            it.next();
        }
        if it.next()? != ':' {
            return None;
        }
        while it.peek().is_some_and(|c| c.is_whitespace()) {
            it.next();
        }
        let v = if *it.peek()? == '"' {
            V::S(string(&mut it)?)
        } else {
            let mut t = String::new();
            while it.peek().is_some_and(|c| *c != ',' && *c != '}') {
                t.push(it.next()?);
            }
            V::N(t.trim().parse().unwrap_or(f64::NAN))
        };
        m.insert(key, v);
    }
}

fn string(it: &mut std::iter::Peekable<std::str::Chars>) -> Option<String> {
    it.next()?;
    let mut s = String::new();
    loop {
        match it.next()? {
            '"' => return Some(s),
            '\\' => s.push(match it.next()? {
                'n' => '\n',
                't' => '\t',
                c => c,
            }),
            c => s.push(c),
        }
    }
}

pub fn num(o: &Obj, k: &str) -> Option<f64> {
    match o.get(k)? {
        V::N(n) if n.is_finite() => Some(*n),
        _ => None,
    }
}

pub fn text(o: &Obj, k: &str) -> Option<String> {
    match o.get(k)? {
        V::S(s) => Some(s.clone()),
        _ => None,
    }
}

pub fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', " ")
}
