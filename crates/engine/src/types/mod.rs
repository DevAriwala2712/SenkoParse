//! Values, typed column vectors, NULL flags, decimal helpers, dates.
use crate::{err, Result};
use std::sync::Arc;

pub const POW10: [i64; 19] = [
    1,
    10,
    100,
    1_000,
    10_000,
    100_000,
    1_000_000,
    10_000_000,
    100_000_000,
    1_000_000_000,
    10_000_000_000,
    100_000_000_000,
    1_000_000_000_000,
    10_000_000_000_000,
    100_000_000_000_000,
    1_000_000_000_000_000,
    10_000_000_000_000_000,
    100_000_000_000_000_000,
    1_000_000_000_000_000_000,
];
pub const MAX_SCALE: u8 = 18;

/// Sentinel row index meaning "no row" (NULL) in `Column::take_opt`.
pub const NONE_IDX: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ty {
    Bool,
    I64,
    F64,
    /// Scaled i64: value = raw / 10^scale. Sums are exact and merge-order independent.
    Dec(u8),
    Str,
    Date,
}

impl Ty {
    pub fn parse(s: &str) -> Option<Ty> {
        let s = s.trim().to_ascii_lowercase();
        match s.as_str() {
            "i64" => Some(Ty::I64),
            "f64" => Some(Ty::F64),
            "str" => Some(Ty::Str),
            "date" => Some(Ty::Date),
            _ => {
                let inner = s.strip_prefix("decimal(")?.strip_suffix(')')?;
                let sc: u8 = inner.trim().parse().ok()?;
                (sc <= MAX_SCALE).then_some(Ty::Dec(sc))
            }
        }
    }
    pub fn name(&self) -> String {
        match self {
            Ty::Bool => "bool".into(),
            Ty::I64 => "i64".into(),
            Ty::F64 => "f64".into(),
            Ty::Dec(s) => format!("decimal({s})"),
            Ty::Str => "str".into(),
            Ty::Date => "date".into(),
        }
    }
    pub fn is_numeric(&self) -> bool {
        matches!(self, Ty::I64 | Ty::F64 | Ty::Dec(_))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    I64(i64),
    F64(f64),
    Dec(i64, u8),
    Str(String),
    Date(i32),
}

// ---------------------------------------------------------------- dates

pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

/// Strict `YYYY-MM-DD`.
pub fn parse_date(b: &[u8]) -> Option<i32> {
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let num = |s: &[u8]| -> Option<u32> {
        let mut v = 0u32;
        for &c in s {
            let d = c.wrapping_sub(b'0');
            if d > 9 {
                return None;
            }
            v = v * 10 + d as u32;
        }
        Some(v)
    };
    let (y, m, d) = (num(&b[0..4])?, num(&b[5..7])?, num(&b[8..10])?);
    if !(1..=12).contains(&m) || d < 1 {
        return None;
    }
    let days = days_from_civil(y as i64, m, d);
    // round trip rejects e.g. Feb 30
    (civil_from_days(days) == (y as i64, m, d)).then_some(days as i32)
}

fn push_u128(out: &mut Vec<u8>, mut n: u128, min_width: usize) {
    let mut tmp = [0u8; 40];
    let mut i = tmp.len();
    loop {
        i -= 1;
        tmp[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    while tmp.len() - i < min_width {
        i -= 1;
        tmp[i] = b'0';
    }
    out.extend_from_slice(&tmp[i..]);
}

pub fn fmt_date(days: i32, out: &mut Vec<u8>) {
    let (y, m, d) = civil_from_days(days as i64);
    if y < 0 {
        out.push(b'-');
    }
    push_u128(out, y.unsigned_abs() as u128, 4);
    out.push(b'-');
    push_u128(out, m as u128, 2);
    out.push(b'-');
    push_u128(out, d as u128, 2);
}

pub fn fmt_dec(v: i64, scale: u8, out: &mut Vec<u8>) {
    if v < 0 {
        out.push(b'-');
    }
    let a = (v as i128).unsigned_abs();
    if scale == 0 {
        push_u128(out, a, 1);
        return;
    }
    let p = POW10[scale as usize] as u128;
    push_u128(out, a / p, 1);
    out.push(b'.');
    push_u128(out, a % p, scale as usize);
}

// ---------------------------------------------------------------- columns

/// Variable-length strings: `offs.len() == n + 1`.
#[derive(Clone, Debug)]
pub struct StrCol {
    pub offs: Vec<usize>,
    pub data: Vec<u8>,
}

impl Default for StrCol {
    fn default() -> Self {
        StrCol { offs: vec![0], data: Vec::new() }
    }
}

impl StrCol {
    pub fn len(&self) -> usize {
        self.offs.len() - 1
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    #[inline]
    pub fn get(&self, i: usize) -> &[u8] {
        &self.data[self.offs[i]..self.offs[i + 1]]
    }
    pub fn push(&mut self, b: &[u8]) {
        self.data.extend_from_slice(b);
        self.offs.push(self.data.len());
    }
}

#[derive(Clone, Debug)]
pub enum ColData {
    Bool(Vec<bool>),
    I64(Vec<i64>),
    F64(Vec<f64>),
    Dec(Vec<i64>, u8),
    Date(Vec<i32>),
    Str(StrCol),
}

/// A column plus optional NULL flags (`nulls[i] == true` means NULL). Slots under a NULL hold the
/// type's default value so kernels never see junk.
#[derive(Clone, Debug)]
pub struct Column {
    pub data: ColData,
    pub nulls: Option<Vec<bool>>,
}

macro_rules! gather {
    ($v:expr, $idx:expr) => {
        $idx.iter().map(|&i| $v[i as usize]).collect::<Vec<_>>()
    };
}
macro_rules! gather_opt {
    ($v:expr, $idx:expr, $def:expr) => {
        $idx.iter().map(|&i| if i == NONE_IDX { $def } else { $v[i as usize] }).collect::<Vec<_>>()
    };
}

impl Column {
    pub fn new(data: ColData, nulls: Option<Vec<bool>>) -> Column {
        Column { data, nulls }
    }

    pub fn empty(ty: Ty) -> Column {
        let data = match ty {
            Ty::Bool => ColData::Bool(vec![]),
            Ty::I64 => ColData::I64(vec![]),
            Ty::F64 => ColData::F64(vec![]),
            Ty::Dec(s) => ColData::Dec(vec![], s),
            Ty::Date => ColData::Date(vec![]),
            Ty::Str => ColData::Str(StrCol::default()),
        };
        Column { data, nulls: None }
    }

    pub fn ty(&self) -> Ty {
        match &self.data {
            ColData::Bool(_) => Ty::Bool,
            ColData::I64(_) => Ty::I64,
            ColData::F64(_) => Ty::F64,
            ColData::Dec(_, s) => Ty::Dec(*s),
            ColData::Date(_) => Ty::Date,
            ColData::Str(_) => Ty::Str,
        }
    }

    pub fn len(&self) -> usize {
        match &self.data {
            ColData::Bool(v) => v.len(),
            ColData::I64(v) => v.len(),
            ColData::F64(v) => v.len(),
            ColData::Dec(v, _) => v.len(),
            ColData::Date(v) => v.len(),
            ColData::Str(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn is_null(&self, i: usize) -> bool {
        match &self.nulls {
            None => false,
            Some(n) => n[i],
        }
    }

    pub fn constant(v: &Value, ty: Ty, n: usize) -> Column {
        let nulls = matches!(v, Value::Null).then(|| vec![true; n]);
        let data = match (ty, v) {
            (Ty::Bool, Value::Bool(b)) => ColData::Bool(vec![*b; n]),
            (Ty::Bool, _) => ColData::Bool(vec![false; n]),
            (Ty::I64, Value::I64(x)) => ColData::I64(vec![*x; n]),
            (Ty::I64, _) => ColData::I64(vec![0; n]),
            (Ty::F64, Value::F64(x)) => ColData::F64(vec![*x; n]),
            (Ty::F64, _) => ColData::F64(vec![0.0; n]),
            (Ty::Dec(s), Value::Dec(x, _)) => ColData::Dec(vec![*x; n], s),
            (Ty::Dec(s), _) => ColData::Dec(vec![0; n], s),
            (Ty::Date, Value::Date(x)) => ColData::Date(vec![*x; n]),
            (Ty::Date, _) => ColData::Date(vec![0; n]),
            (Ty::Str, Value::Str(x)) => {
                let mut s = StrCol::default();
                for _ in 0..n {
                    s.push(x.as_bytes());
                }
                ColData::Str(s)
            }
            (Ty::Str, _) => {
                let mut s = StrCol::default();
                for _ in 0..n {
                    s.push(b"");
                }
                ColData::Str(s)
            }
        };
        Column { data, nulls }
    }

    pub fn value(&self, i: usize) -> Value {
        if self.is_null(i) {
            return Value::Null;
        }
        match &self.data {
            ColData::Bool(v) => Value::Bool(v[i]),
            ColData::I64(v) => Value::I64(v[i]),
            ColData::F64(v) => Value::F64(v[i]),
            ColData::Dec(v, s) => Value::Dec(v[i], *s),
            ColData::Date(v) => Value::Date(v[i]),
            ColData::Str(v) => Value::Str(String::from_utf8_lossy(v.get(i)).into_owned()),
        }
    }

    /// Integer-like payload (I64, Dec raw, Date days, Bool).
    #[inline]
    pub fn int_at(&self, i: usize) -> i64 {
        match &self.data {
            ColData::I64(v) | ColData::Dec(v, _) => v[i],
            ColData::Date(v) => v[i] as i64,
            ColData::Bool(v) => v[i] as i64,
            _ => unreachable!("int_at on non-integer column"),
        }
    }

    pub fn push_null(&mut self) {
        let n = self.len();
        match &mut self.data {
            ColData::Bool(v) => v.push(false),
            ColData::I64(v) => v.push(0),
            ColData::F64(v) => v.push(0.0),
            ColData::Dec(v, _) => v.push(0),
            ColData::Date(v) => v.push(0),
            ColData::Str(v) => v.push(b""),
        }
        self.nulls.get_or_insert_with(|| vec![false; n]).push(true);
    }

    pub fn push_value(&mut self, v: &Value) {
        if matches!(v, Value::Null) {
            return self.push_null();
        }
        match (&mut self.data, v) {
            (ColData::Bool(c), Value::Bool(x)) => c.push(*x),
            (ColData::I64(c), Value::I64(x)) => c.push(*x),
            (ColData::F64(c), Value::F64(x)) => c.push(*x),
            (ColData::Dec(c, _), Value::Dec(x, _)) => c.push(*x),
            (ColData::Date(c), Value::Date(x)) => c.push(*x),
            (ColData::Str(c), Value::Str(x)) => c.push(x.as_bytes()),
            _ => unreachable!("push_value type mismatch"),
        }
        if let Some(n) = &mut self.nulls {
            n.push(false);
        }
    }

    /// Append row `i` of `src` (same type).
    pub fn push_from(&mut self, src: &Column, i: usize) {
        if src.is_null(i) {
            return self.push_null();
        }
        match (&mut self.data, &src.data) {
            (ColData::Bool(c), ColData::Bool(s)) => c.push(s[i]),
            (ColData::I64(c), ColData::I64(s)) => c.push(s[i]),
            (ColData::F64(c), ColData::F64(s)) => c.push(s[i]),
            (ColData::Dec(c, _), ColData::Dec(s, _)) => c.push(s[i]),
            (ColData::Date(c), ColData::Date(s)) => c.push(s[i]),
            (ColData::Str(c), ColData::Str(s)) => c.push(s.get(i)),
            _ => unreachable!("push_from type mismatch"),
        }
        if let Some(n) = &mut self.nulls {
            n.push(false);
        }
    }

    pub fn append(&mut self, o: &Column) {
        let n = self.len();
        match (&mut self.data, &o.data) {
            (ColData::Bool(c), ColData::Bool(s)) => c.extend_from_slice(s),
            (ColData::I64(c), ColData::I64(s)) => c.extend_from_slice(s),
            (ColData::F64(c), ColData::F64(s)) => c.extend_from_slice(s),
            (ColData::Dec(c, _), ColData::Dec(s, _)) => c.extend_from_slice(s),
            (ColData::Date(c), ColData::Date(s)) => c.extend_from_slice(s),
            (ColData::Str(c), ColData::Str(s)) => {
                for i in 0..s.len() {
                    c.push(s.get(i));
                }
            }
            _ => unreachable!("append type mismatch"),
        }
        match (&mut self.nulls, &o.nulls) {
            (None, None) => {}
            (Some(a), None) => a.extend(std::iter::repeat(false).take(o.len())),
            (Some(a), Some(b)) => a.extend_from_slice(b),
            (None, Some(b)) => {
                let mut a = vec![false; n];
                a.extend_from_slice(b);
                self.nulls = Some(a);
            }
        }
    }

    /// Gather rows by index.
    pub fn take(&self, idx: &[u32]) -> Column {
        let data = match &self.data {
            ColData::Bool(v) => ColData::Bool(gather!(v, idx)),
            ColData::I64(v) => ColData::I64(gather!(v, idx)),
            ColData::F64(v) => ColData::F64(gather!(v, idx)),
            ColData::Dec(v, s) => ColData::Dec(gather!(v, idx), *s),
            ColData::Date(v) => ColData::Date(gather!(v, idx)),
            ColData::Str(v) => {
                let mut o = StrCol::default();
                for &i in idx {
                    o.push(v.get(i as usize));
                }
                ColData::Str(o)
            }
        };
        let nulls = self.nulls.as_ref().map(|n| gather!(n, idx));
        Column { data, nulls }
    }

    /// Gather rows by index; `NONE_IDX` yields NULL (used for LEFT JOIN padding).
    pub fn take_opt(&self, idx: &[u32]) -> Column {
        let data = match &self.data {
            ColData::Bool(v) => ColData::Bool(gather_opt!(v, idx, false)),
            ColData::I64(v) => ColData::I64(gather_opt!(v, idx, 0)),
            ColData::F64(v) => ColData::F64(gather_opt!(v, idx, 0.0)),
            ColData::Dec(v, s) => ColData::Dec(gather_opt!(v, idx, 0), *s),
            ColData::Date(v) => ColData::Date(gather_opt!(v, idx, 0)),
            ColData::Str(v) => {
                let mut o = StrCol::default();
                for &i in idx {
                    if i == NONE_IDX {
                        o.push(b"");
                    } else {
                        o.push(v.get(i as usize));
                    }
                }
                ColData::Str(o)
            }
        };
        let nulls = if idx.contains(&NONE_IDX) || self.nulls.is_some() {
            Some(
                idx.iter()
                    .map(|&i| i == NONE_IDX || self.nulls.as_ref().map_or(false, |n| n[i as usize]))
                    .collect(),
            )
        } else {
            None
        };
        Column { data, nulls }
    }

    pub fn slice(&self, start: usize, len: usize) -> Column {
        let idx: Vec<u32> = (start..start + len).map(|i| i as u32).collect();
        self.take(&idx)
    }

    /// Append a canonical byte encoding of row `i` (equal values encode equally; NULL is its own code).
    #[inline]
    pub fn encode_key(&self, i: usize, buf: &mut Vec<u8>) {
        if self.is_null(i) {
            buf.push(0);
            return;
        }
        buf.push(1);
        match &self.data {
            ColData::Bool(v) => buf.push(v[i] as u8),
            ColData::I64(v) | ColData::Dec(v, _) => buf.extend_from_slice(&v[i].to_le_bytes()),
            ColData::F64(v) => {
                let x = if v[i] == 0.0 { 0.0 } else { v[i] }; // -0.0 == 0.0
                buf.extend_from_slice(&x.to_bits().to_le_bytes())
            }
            ColData::Date(v) => buf.extend_from_slice(&v[i].to_le_bytes()),
            ColData::Str(v) => {
                let s = v.get(i);
                buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
                buf.extend_from_slice(s);
            }
        }
    }

    /// Total order used by ORDER BY: NULL sorts first (SQLite semantics).
    pub fn cmp_rows(&self, i: usize, j: usize) -> std::cmp::Ordering {
        use std::cmp::Ordering::*;
        match (self.is_null(i), self.is_null(j)) {
            (true, true) => return Equal,
            (true, false) => return Less,
            (false, true) => return Greater,
            _ => {}
        }
        match &self.data {
            ColData::Bool(v) => v[i].cmp(&v[j]),
            ColData::I64(v) | ColData::Dec(v, _) => v[i].cmp(&v[j]),
            ColData::F64(v) => v[i].partial_cmp(&v[j]).unwrap_or(Equal),
            ColData::Date(v) => v[i].cmp(&v[j]),
            ColData::Str(v) => v.get(i).cmp(v.get(j)),
        }
    }
}

#[derive(Clone)]
pub struct Batch {
    pub cols: Vec<Arc<Column>>,
    pub len: usize,
}

impl Batch {
    pub fn take(&self, idx: &[u32]) -> Batch {
        Batch { cols: self.cols.iter().map(|c| Arc::new(c.take(idx))).collect(), len: idx.len() }
    }
}

pub fn checked_i64(v: i128, what: &str) -> Result<i64> {
    i64::try_from(v).map_err(|_| err!("integer overflow in {what}"))
}
