//! Single-threaded memory-mapped CSV scanner: RFC 4180 quoting, CRLF, header row, missing final
//! newline. Only referenced columns are parsed; the rest are skipped field by field (still
//! quote-aware, so an embedded newline or comma inside quotes can never split a row wrongly).
//!
//! Empty field (quoted or not) = NULL, for every type.
use super::TableDef;
use crate::types::*;
use crate::{err, Result};
use memchr::{memchr, memchr2};
use memmap2::Mmap;
use std::path::Path;
use rayon::prelude::*;
use std::sync::Arc;

pub const BATCH_ROWS: usize = 64 * 1024;

pub struct CsvReader {
    mmap: Arc<Mmap>,
    /// Exclusive end of this reader's byte range; always a row start or the end of the file.
    end: usize,
    pos: usize,
    ncols: usize,
    /// file column index -> output position
    wanted: Vec<Option<usize>>,
    outs: Vec<(String, Ty)>,
    row: u64,
    path: String,
    /// No `"` byte anywhere after the header: rows are exactly the `\n`-delimited lines and fields the
    /// `,`-delimited pieces, so the quote-aware splitter can be replaced by the fast path below.
    /// Always false for the `--no-opt` reference path.
    quote_free: bool,
    /// Highest file column index that must be materialized; fields after it are never located.
    last_needed: Option<usize>,
}

struct Fld {
    start: usize,
    end: usize,
    esc: bool,
    next: usize,
    eor: bool,
}

#[inline]
fn next_field(d: &[u8], p: usize) -> std::result::Result<Fld, &'static str> {
    let len = d.len();
    if p < len && d[p] == b'"' {
        let mut q = p + 1;
        let mut esc = false;
        loop {
            let at = match memchr(b'"', &d[q.min(len)..]) {
                None => return Err("unterminated quoted field"),
                Some(k) => q + k,
            };
            if at + 1 < len && d[at + 1] == b'"' {
                esc = true;
                q = at + 2;
                continue;
            }
            let after = at + 1;
            if after >= len {
                return Ok(Fld { start: p + 1, end: at, esc, next: len, eor: true });
            }
            return match d[after] {
                b',' => Ok(Fld { start: p + 1, end: at, esc, next: after + 1, eor: false }),
                b'\n' => Ok(Fld { start: p + 1, end: at, esc, next: after + 1, eor: true }),
                b'\r' if after + 1 >= len => Ok(Fld { start: p + 1, end: at, esc, next: len, eor: true }),
                b'\r' if d[after + 1] == b'\n' => Ok(Fld { start: p + 1, end: at, esc, next: after + 2, eor: true }),
                _ => Err("unexpected character after closing quote"),
            };
        }
    }
    match memchr2(b',', b'\n', &d[p.min(len)..]) {
        None => {
            let mut end = len;
            if end > p && d[end - 1] == b'\r' {
                end -= 1;
            }
            no_stray_quote(d, p, end)?;
            Ok(Fld { start: p, end, esc: false, next: len, eor: true })
        }
        Some(k) => {
            let at = p + k;
            if d[at] == b',' {
                no_stray_quote(d, p, at)?;
                Ok(Fld { start: p, end: at, esc: false, next: at + 1, eor: false })
            } else {
                let mut end = at;
                if end > p && d[end - 1] == b'\r' {
                    end -= 1;
                }
                no_stray_quote(d, p, end)?;
                Ok(Fld { start: p, end, esc: false, next: at + 1, eor: true })
            }
        }
    }
}

/// A `"` inside an unquoted field is not RFC 4180. Rejecting it (instead of treating it as a literal)
/// guarantees every quote byte is structural, which is what makes parallel chunk splitting by quote
/// parity exact.
#[inline]
fn no_stray_quote(d: &[u8], start: usize, end: usize) -> std::result::Result<(), &'static str> {
    if start < end && memchr(b'"', &d[start..end]).is_some() {
        return Err("unexpected quote inside an unquoted field");
    }
    Ok(())
}

fn unescape(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        out.push(b[i]);
        if b[i] == b'"' && i + 1 < b.len() && b[i + 1] == b'"' {
            i += 1;
        }
        i += 1;
    }
    out
}

pub fn parse_i64(b: &[u8]) -> Option<i64> {
    let (neg, digs) = match b.first()? {
        b'-' => (true, &b[1..]),
        b'+' => (false, &b[1..]),
        _ => (false, b),
    };
    if digs.is_empty() || digs.len() > 19 {
        return None;
    }
    let mut v: u64 = 0;
    for &c in digs {
        let d = c.wrapping_sub(b'0');
        if d > 9 {
            return None;
        }
        v = v.checked_mul(10)?.checked_add(d as u64)?;
    }
    if neg {
        (v <= 1u64 << 63).then(|| (v as i64).wrapping_neg())
    } else {
        i64::try_from(v).ok()
    }
}

/// Exact decimal parse to a scaled i64. Extra fractional digits are allowed only if they are zeros.
pub fn parse_dec(b: &[u8], scale: u8) -> Option<i64> {
    let (neg, rest) = match b.first()? {
        b'-' => (true, &b[1..]),
        b'+' => (false, &b[1..]),
        _ => (false, b),
    };
    let (ip, fp) = match memchr(b'.', rest) {
        Some(k) => (&rest[..k], &rest[k + 1..]),
        None => (rest, &rest[0..0]),
    };
    if ip.is_empty() && fp.is_empty() {
        return None;
    }
    let mut v: i128 = 0;
    for &c in ip {
        let d = c.wrapping_sub(b'0');
        if d > 9 {
            return None;
        }
        v = v.checked_mul(10)?.checked_add(d as i128)?;
    }
    for k in 0..fp.len() {
        let d = fp[k].wrapping_sub(b'0');
        if d > 9 {
            return None;
        }
        if k < scale as usize {
            v = v.checked_mul(10)?.checked_add(d as i128)?;
        } else if d != 0 {
            return None; // would lose precision
        }
    }
    for _ in fp.len().min(scale as usize)..scale as usize {
        v = v.checked_mul(10)?;
    }
    if neg {
        v = -v;
    }
    i64::try_from(v).ok()
}

struct Builder {
    ty: Ty,
    data: ColData,
    nulls: Vec<bool>,
    any_null: bool,
}

impl Builder {
    fn new(ty: Ty) -> Builder {
        Builder { ty, data: Column::empty(ty).data, nulls: Vec::with_capacity(BATCH_ROWS), any_null: false }
    }

    #[inline]
    fn push(&mut self, b: &[u8], esc: bool) -> std::result::Result<(), String> {
        if b.is_empty() {
            self.any_null = true;
            self.nulls.push(true);
            match &mut self.data {
                ColData::I64(v) | ColData::Dec(v, _) => v.push(0),
                ColData::F64(v) => v.push(0.0),
                ColData::Date(v) => v.push(0),
                ColData::Str(v) => v.push(b""),
                ColData::Bool(v) => v.push(false),
            }
            return Ok(());
        }
        self.nulls.push(false);
        let bad = |ty: &str| format!("invalid {ty} value '{}'", String::from_utf8_lossy(b));
        match &mut self.data {
            ColData::I64(v) => v.push(parse_i64(b).ok_or_else(|| bad("i64"))?),
            ColData::Dec(v, s) => v.push(parse_dec(b, *s).ok_or_else(|| bad(&format!("decimal({s})")))?),
            ColData::F64(v) => {
                let x = std::str::from_utf8(b).ok().and_then(|s| s.parse::<f64>().ok()).filter(|x| x.is_finite());
                v.push(x.ok_or_else(|| bad("f64"))?)
            }
            ColData::Date(v) => v.push(parse_date(b).ok_or_else(|| bad("date"))?),
            ColData::Str(v) => {
                if esc {
                    v.push(&unescape(b))
                } else {
                    v.push(b)
                }
            }
            ColData::Bool(_) => unreachable!(),
        }
        Ok(())
    }

    fn finish(&mut self) -> Column {
        let data = std::mem::replace(&mut self.data, Column::empty(self.ty).data);
        let nulls = std::mem::take(&mut self.nulls);
        let c = Column { data, nulls: self.any_null.then_some(nulls) };
        self.any_null = false;
        c
    }
}

/// An opened, header-parsed CSV file that can be read whole or in independent row-aligned chunks.
pub struct CsvSource {
    mmap: Arc<Mmap>,
    data_start: usize,
    ncols: usize,
    wanted: Vec<Option<usize>>,
    outs: Vec<(String, Ty)>,
    path: String,
    reference: bool,
}

impl CsvSource {
    /// `needed`: schema column indices to materialize, output in that order.
    /// `reference`: disable all fast paths (the `--no-opt` generic scanner).
    pub fn open(table: &TableDef, needed: &[usize], reference: bool) -> Result<CsvSource> {
        let path = &table.path;
        let pstr = path.display().to_string();
        let file = std::fs::File::open(path).map_err(|e| err!("cannot open {pstr}: {e}"))?;
        if file.metadata().map_err(|e| err!("{pstr}: {e}"))?.len() == 0 {
            return Err(err!("{pstr}: empty file (missing header row)"));
        }
        // Safety: the file is mapped read-only and treated as immutable for the query's lifetime.
        let mmap = unsafe { Mmap::map(&file) }.map_err(|e| err!("cannot mmap {pstr}: {e}"))?;
        #[cfg(unix)]
        let _ = mmap.advise(memmap2::Advice::Sequential);
        let d: &[u8] = &mmap;
        let mut pos = if d.starts_with(&[0xEF, 0xBB, 0xBF]) { 3 } else { 0 };
        let mut header = Vec::new();
        loop {
            let f = next_field(d, pos).map_err(|m| err!("{pstr}: header: {m}"))?;
            let raw = &d[f.start..f.end];
            let name = if f.esc { unescape(raw) } else { raw.to_vec() };
            header.push(String::from_utf8_lossy(&name).trim().to_ascii_lowercase());
            pos = f.next;
            if f.eor {
                break;
            }
        }
        let mut wanted = vec![None; header.len()];
        let mut seen = vec![false; table.cols.len()];
        for (fi, h) in header.iter().enumerate() {
            let si = table
                .cols
                .iter()
                .position(|(n, _)| n == h)
                .ok_or_else(|| err!("{pstr}: header column '{h}' is not in schema.toml for table '{}'", table.name))?;
            if seen[si] {
                return Err(err!("{pstr}: duplicate header column '{h}'"));
            }
            seen[si] = true;
            wanted[fi] = needed.iter().position(|&n| n == si);
        }
        if let Some(si) = seen.iter().position(|s| !s) {
            return Err(err!("{pstr}: schema column '{}' missing from header", table.cols[si].0));
        }
        let outs = needed.iter().map(|&i| table.cols[i].clone()).collect();
        Ok(CsvSource { mmap: Arc::new(mmap), data_start: pos, ncols: header.len(), wanted, outs, path: pstr, reference })
    }

    pub fn len(&self) -> usize {
        self.mmap.len()
    }

    /// Reader over rows in `[start, end)`; both must be row starts (or the end of the file).
    pub fn reader(&self, start: usize, end: usize) -> CsvReader {
        let d: &[u8] = &self.mmap;
        // One SIMD pass for a quote byte; stops at the first hit, so ranges with quotes pay almost nothing.
        let quote_free = !self.reference && memchr(b'"', &d[start.min(end)..end]).is_none();
        let last_needed = self.wanted.iter().rposition(|w| w.is_some());
        CsvReader {
            mmap: self.mmap.clone(),
            end,
            pos: start,
            ncols: self.ncols,
            wanted: self.wanted.clone(),
            outs: self.outs.clone(),
            row: 0,
            path: self.path.clone(),
            quote_free,
            last_needed,
        }
    }

    pub fn whole(&self) -> CsvReader {
        self.reader(self.data_start, self.mmap.len())
    }

    /// Split the data region into row-aligned ranges of about `chunk_bytes` each.
    ///
    /// Exactness: a split point must be a row start, i.e. a newline that is not inside a quoted field.
    /// Every `"` byte is structural (stray quotes are rejected by the scanner, `""` toggles twice), so the
    /// quote state at any offset is the parity of the quote bytes before it. Quote bytes are counted per
    /// raw range in parallel, prefix-XORed, and then each split scans forward from its raw boundary with
    /// that starting state to the first newline outside quotes. The result depends only on the file and
    /// `chunk_bytes`, never on the number of threads.
    pub fn plan_chunks(&self, chunk_bytes: usize) -> Vec<(usize, usize)> {
        let d: &[u8] = &self.mmap;
        let (start, len) = (self.data_start, d.len());
        if start >= len {
            return vec![];
        }
        let cb = chunk_bytes.max(1);
        let nraw = (len - start).div_ceil(cb);
        if nraw <= 1 {
            return vec![(start, len)];
        }
        let counts: Vec<usize> = (0..nraw)
            .into_par_iter()
            .map(|i| {
                let (a, b) = (start + i * cb, (start + (i + 1) * cb).min(len));
                memchr::memchr_iter(b'"', &d[a..b]).count()
            })
            .collect();
        let mut inside_at = Vec::with_capacity(nraw);
        let mut par = false;
        for c in &counts {
            inside_at.push(par);
            par ^= c & 1 == 1;
        }
        let mut starts = vec![start];
        for i in 1..nraw {
            let cand = start + i * cb;
            let r = row_start_at_or_after(d, cand, inside_at[i]);
            if r < len && r > *starts.last().unwrap() {
                starts.push(r);
            }
        }
        (0..starts.len()).map(|i| (starts[i], starts.get(i + 1).copied().unwrap_or(len))).collect()
    }
}

/// First row start at or after `cand`, given whether `cand` lies inside a quoted field.
fn row_start_at_or_after(d: &[u8], cand: usize, inside0: bool) -> usize {
    if !inside0 && d[cand - 1] == b'\n' {
        return cand;
    }
    let mut inside = inside0;
    let mut p = cand;
    loop {
        match memchr2(b'"', b'\n', &d[p..]) {
            None => return d.len(),
            Some(k) => {
                let at = p + k;
                if d[at] == b'"' {
                    inside = !inside;
                } else if !inside {
                    return at + 1;
                }
                p = at + 1;
            }
        }
    }
}

impl CsvReader {
    pub fn next_batch(&mut self) -> Result<Option<crate::types::Batch>> {
        if self.quote_free {
            return self.next_batch_quote_free();
        }
        let d: &[u8] = &self.mmap[..self.end];
        if self.pos >= d.len() {
            return Ok(None);
        }
        let mut bs: Vec<Builder> = self.outs.iter().map(|(_, t)| Builder::new(*t)).collect();
        let mut rows = 0usize;
        let mut pos = self.pos;
        while rows < BATCH_ROWS && pos < d.len() {
            // skip blank lines (only unambiguous when rows have more than one column)
            if self.ncols > 1 {
                if d[pos] == b'\n' {
                    pos += 1;
                    continue;
                }
                if d[pos] == b'\r' && pos + 1 < d.len() && d[pos + 1] == b'\n' {
                    pos += 2;
                    continue;
                }
            }
            let mut f = 0usize;
            loop {
                let fld = next_field(d, pos).map_err(|m| err!("{}: row {}: {m}", self.path, self.row + 2))?;
                pos = fld.next;
                if f < self.ncols {
                    if let Some(o) = self.wanted[f] {
                        bs[o].push(&d[fld.start..fld.end], fld.esc).map_err(|m| {
                            err!("{}: row {}, column '{}': {m}", self.path, self.row + 2, self.outs[o].0)
                        })?;
                    }
                }
                f += 1;
                if fld.eor {
                    break;
                }
            }
            if f != self.ncols {
                return Err(err!("{}: row {}: expected {} fields, found {f}", self.path, self.row + 2, self.ncols));
            }
            rows += 1;
            self.row += 1;
        }
        self.pos = pos;
        if rows == 0 {
            return Ok(None);
        }
        let cols = bs.iter_mut().map(|b| Arc::new(b.finish())).collect();
        Ok(Some(crate::types::Batch { cols, len: rows }))
    }
}

/// Number of `,` bytes in a line (written as a plain loop so it auto-vectorizes).
#[inline]
fn count_commas(line: &[u8]) -> usize {
    line.iter().map(|&b| (b == b',') as usize).sum()
}

impl CsvReader {
    /// Fast path, valid only when the file has no quote bytes (see `quote_free`). Must behave exactly
    /// like the generic path, including errors, so ragged rows are still detected even when the extra
    /// fields are never parsed: the comma count of every line is checked (SIMD-friendly), and only the
    /// fields up to the last needed column are located and parsed.
    fn next_batch_quote_free(&mut self) -> Result<Option<crate::types::Batch>> {
        let d: &[u8] = &self.mmap[..self.end];
        if self.pos >= d.len() {
            return Ok(None);
        }
        let mut bs: Vec<Builder> = self.outs.iter().map(|(_, t)| Builder::new(*t)).collect();
        let mut rows = 0usize;
        let mut pos = self.pos;
        while rows < BATCH_ROWS && pos < d.len() {
            let (end, next, has_nl) = match memchr(b'\n', &d[pos..]) {
                Some(k) => (pos + k, pos + k + 1, true),
                None => (d.len(), d.len(), false),
            };
            let raw = &d[pos..end];
            // same blank-line rule as the generic path: "" or "\r" followed by a newline, when rows have >1 column
            if self.ncols > 1 && (raw.is_empty() || (has_nl && raw == b"\r")) {
                pos = next;
                continue;
            }
            let line = match raw.last() {
                Some(b'\r') => &raw[..raw.len() - 1],
                _ => raw,
            };
            let nfields = count_commas(line) + 1;
            if nfields != self.ncols {
                return Err(err!("{}: row {}: expected {} fields, found {nfields}", self.path, self.row + 2, self.ncols));
            }
            if let Some(last) = self.last_needed {
                let mut p = 0usize;
                for f in 0..=last {
                    // the comma count above guarantees a comma exists for every field except the last
                    let e = if f + 1 == self.ncols { line.len() } else { p + memchr(b',', &line[p..]).unwrap() };
                    if let Some(o) = self.wanted[f] {
                        bs[o].push(&line[p..e], false).map_err(|m| {
                            err!("{}: row {}, column '{}': {m}", self.path, self.row + 2, self.outs[o].0)
                        })?;
                    }
                    p = e + 1;
                }
            }
            rows += 1;
            self.row += 1;
            pos = next;
        }
        self.pos = pos;
        if rows == 0 {
            return Ok(None);
        }
        let cols = bs.iter_mut().map(|b| Arc::new(b.finish())).collect();
        Ok(Some(crate::types::Batch { cols, len: rows }))
    }
}

#[allow(dead_code)]
fn _assert_path(_: &Path) {}
