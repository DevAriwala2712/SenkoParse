//! CSV result writer: header row, RFC 4180 quoting, NULL = empty field.
use crate::exec::Op;
use crate::types::*;
use crate::Result;
use std::io::Write;

fn field(out: &mut Vec<u8>, b: &[u8]) {
    if b.iter().any(|&c| matches!(c, b',' | b'"' | b'\n' | b'\r')) {
        out.push(b'"');
        for &c in b {
            if c == b'"' {
                out.push(b'"');
            }
            out.push(c);
        }
        out.push(b'"');
    } else {
        out.extend_from_slice(b);
    }
}

/// Append a signed integer without allocating.
#[inline]
fn push_i64(buf: &mut Vec<u8>, v: i64) {
    let mut tmp = [0u8; 20];
    let mut i = tmp.len();
    let mut n = v.unsigned_abs();
    loop {
        i -= 1;
        tmp[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    if v < 0 {
        i -= 1;
        tmp[i] = b'-';
    }
    buf.extend_from_slice(&tmp[i..]);
}

pub fn write_csv<W: Write>(names: &[String], op: &mut dyn Op, w: &mut W) -> Result<usize> {
    write_csv_opts(names, op, w, false)
}

/// `flush_per_batch`: write and flush after every input batch (stream mode: results appear as input arrives).
pub fn write_csv_opts<W: Write>(names: &[String], op: &mut dyn Op, w: &mut W, flush_per_batch: bool) -> Result<usize> {
    let io = |e: std::io::Error| crate::err!("write error: {e}");
    let mut buf: Vec<u8> = Vec::with_capacity(1 << 16);
    for (i, n) in names.iter().enumerate() {
        if i > 0 {
            buf.push(b',');
        }
        field(&mut buf, n.as_bytes());
    }
    buf.push(b'\n');
    let mut rows = 0;
    while let Some(b) = op.next()? {
        for i in 0..b.len {
            for (j, c) in b.cols.iter().enumerate() {
                if j > 0 {
                    buf.push(b',');
                }
                if c.is_null(i) {
                    continue;
                }
                match &c.data {
                    ColData::Bool(v) => buf.push(if v[i] { b'1' } else { b'0' }),
                    ColData::I64(v) => push_i64(&mut buf, v[i]),
                    ColData::F64(v) => {
                        // shortest round-trip representation, written straight into the buffer (no per-cell String)
                        let _ = write!(buf, "{}", v[i]);
                    }
                    ColData::Dec(v, s) => fmt_dec(v[i], *s, &mut buf),
                    ColData::Date(v) => fmt_date(v[i], &mut buf),
                    ColData::Str(v) => field(&mut buf, v.get(i)),
                }
            }
            buf.push(b'\n');
            if buf.len() > (1 << 16) {
                w.write_all(&buf).map_err(io)?;
                buf.clear();
            }
        }
        rows += b.len;
        if flush_per_batch && !buf.is_empty() {
            w.write_all(&buf).map_err(io)?;
            buf.clear();
            w.flush().map_err(io)?;
        }
    }
    w.write_all(&buf).map_err(io)?;
    w.flush().map_err(io)?;
    Ok(rows)
}
