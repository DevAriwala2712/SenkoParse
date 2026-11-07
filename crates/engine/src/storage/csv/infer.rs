//! Type inference for a CSV file that has no `schema.toml` entry (ADR-0007, decision D-5).
//!
//! Validity: each sampled value is classified with the exact parsers `Builder::push` uses, so a value seen in
//! the sample can never fail to parse at scan time. Types only widen (`i64 → f64 → str`, `date → str`), so the
//! chosen type accepts every sampled value. A value after the sample that does not fit is a scan error with a
//! hint (`CsvReader::hint`), never a silent NULL. The sample is a fixed byte prefix of the file, so the result
//! is independent of threads and chunk size.
use super::{next_field, parse_i64, unescape};
use crate::types::{parse_date, Ty};
use crate::{err, Result};
use std::io::Read;
use std::path::Path;

/// Bytes read from the start of the file. Whole rows only, except that the first data row is always read
/// completely even if it is longer.
pub const SAMPLE_BYTES: usize = 1 << 20;

/// What a column has seen so far. `None` = only empty fields (NULL).
fn widen(cur: Option<Ty>, v: Ty) -> Option<Ty> {
    Some(match (cur, v) {
        (None, v) => v,
        (Some(a), b) if a == b => a,
        (Some(Ty::I64), Ty::F64) | (Some(Ty::F64), Ty::I64) => Ty::F64,
        _ => Ty::Str,
    })
}

fn classify(b: &[u8]) -> Ty {
    if parse_i64(b).is_some() {
        Ty::I64
    } else if parse_date(b).is_some() {
        Ty::Date
    } else if std::str::from_utf8(b).ok().and_then(|s| s.parse::<f64>().ok()).is_some_and(|x| x.is_finite()) {
        Ty::F64
    } else {
        Ty::Str
    }
}

/// Header names and inferred types for the CSV at `path`.
pub fn infer_columns(path: &Path) -> Result<Vec<(String, Ty)>> {
    let pstr = path.display().to_string();
    let f = std::fs::File::open(path).map_err(|e| err!("cannot open {pstr}: {e}"))?;
    let mut buf = Vec::with_capacity(SAMPLE_BYTES.min(64 * 1024));
    f.take(SAMPLE_BYTES as u64).read_to_end(&mut buf).map_err(|e| err!("{pstr}: {e}"))?;
    let at_eof = buf.len() < SAMPLE_BYTES;
    infer_from(&buf, at_eof, path)
}

/// `d` = the first bytes of the file; `at_eof` = `d` is the whole file.
fn infer_from(d: &[u8], at_eof: bool, path: &Path) -> Result<Vec<(String, Ty)>> {
    let pstr = path.display().to_string();
    if d.is_empty() {
        return Err(err!("{pstr}: empty file (missing header row)"));
    }
    let mut pos = if d.starts_with(&[0xEF, 0xBB, 0xBF]) { 3 } else { 0 };
    let mut names: Vec<String> = Vec::new();
    loop {
        let f = next_field(d, pos).map_err(|m| err!("{pstr}: header: {m}"))?;
        if !at_eof && f.next >= d.len() && d.last() != Some(&b'\n') {
            return Err(err!("{pstr}: header row longer than {SAMPLE_BYTES} bytes"));
        }
        let raw = &d[f.start..f.end];
        let name = if f.esc { unescape(raw) } else { raw.to_vec() };
        let name = String::from_utf8_lossy(&name).trim().to_ascii_lowercase();
        if name.is_empty() {
            return Err(err!("{pstr}: header column {} has no name", names.len() + 1));
        }
        if names.contains(&name) {
            return Err(err!("{pstr}: duplicate header column '{name}'"));
        }
        names.push(name);
        pos = f.next;
        if f.eor {
            break;
        }
    }
    let mut seen: Vec<Option<Ty>> = vec![None; names.len()];
    // Rows are parsed into `row` and committed only when complete, so a row cut by the sample end is ignored
    // (unless the sample is the whole file, where the scanner's own rules apply and errors are real).
    let mut row: Vec<(usize, usize, bool)> = Vec::with_capacity(names.len());
    let mut nrows = 0usize;
    while pos < d.len() {
        if d[pos] == b'\n' || (d[pos] == b'\r' && d.get(pos + 1) == Some(&b'\n')) {
            pos += if d[pos] == b'\n' { 1 } else { 2 }; // blank line, skipped like the scanner does
            continue;
        }
        row.clear();
        let mut p = pos;
        let mut cut = false;
        loop {
            let f = match next_field(d, p) {
                Ok(f) => f,
                Err(_) if !at_eof => {
                    cut = true;
                    break;
                }
                Err(m) => return Err(err!("{pstr}: row {}: {m}", nrows + 2)),
            };
            row.push((f.start, f.end, f.esc));
            p = f.next;
            if f.eor {
                // A field that ran into the end of a partial sample is not known to be complete.
                if !at_eof && p >= d.len() && d.last() != Some(&b'\n') {
                    cut = true;
                }
                break;
            }
        }
        if cut {
            break;
        }
        if row.len() != names.len() {
            return Err(err!("{pstr}: row {}: expected {} fields, found {}", nrows + 2, names.len(), row.len()));
        }
        for (i, &(s, e, esc)) in row.iter().enumerate() {
            if s == e {
                continue;
            }
            let v = if esc { classify(&unescape(&d[s..e])) } else { classify(&d[s..e]) };
            seen[i] = widen(seen[i], v);
        }
        nrows += 1;
        pos = p;
    }
    Ok(names.into_iter().zip(seen).map(|(n, t)| (n, t.unwrap_or(Ty::Str))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inf(s: &str) -> Vec<(String, Ty)> {
        infer_from(s.as_bytes(), true, Path::new("t.csv")).unwrap()
    }

    #[test]
    fn widening() {
        let c = inf("a,b,c,d,e,f,g\n1,1,x,2024-01-02,1,,2024-01-02\n2,1.5,3,2024-02-03,,,5\n");
        let tys: Vec<Ty> = c.iter().map(|x| x.1).collect();
        assert_eq!(tys, vec![Ty::I64, Ty::F64, Ty::Str, Ty::Date, Ty::I64, Ty::Str, Ty::Str]);
    }

    #[test]
    fn quoted_and_crlf() {
        let c = inf("\"Id\",\"note\"\r\n1,\"a,\r\nb\"\r\n\"2\",\"\"\"q\"\"\"\r\n");
        assert_eq!(c, vec![("id".into(), Ty::I64), ("note".into(), Ty::Str)]);
    }

    #[test]
    fn non_finite_and_overflow() {
        let c = inf("a,b,c\nnan,99999999999999999999,1e3\ninf,1,2\n");
        assert_eq!(c.iter().map(|x| x.1).collect::<Vec<_>>(), vec![Ty::Str, Ty::F64, Ty::F64]);
    }

    #[test]
    fn partial_sample_ignores_cut_row() {
        // The last row is cut mid-field: "12" could be "12x" in the full file, so it must not count.
        let c = infer_from(b"a\n1\n12", false, Path::new("t.csv")).unwrap();
        assert_eq!(c, vec![("a".into(), Ty::I64)]);
        let c = infer_from(b"a\n1\n\"open", false, Path::new("t.csv")).unwrap();
        assert_eq!(c, vec![("a".into(), Ty::I64)]);
    }

    #[test]
    fn header_errors() {
        assert!(infer_from(b"a,a\n1,2\n", true, Path::new("t.csv")).unwrap_err().0.contains("duplicate"));
        assert!(infer_from(b"a,,b\n", true, Path::new("t.csv")).unwrap_err().0.contains("no name"));
        assert!(infer_from(b"", true, Path::new("t.csv")).unwrap_err().0.contains("empty file"));
        assert!(infer_from(b"a,b\n1\n", true, Path::new("t.csv")).unwrap_err().0.contains("expected 2 fields"));
    }
}
