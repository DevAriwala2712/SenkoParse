//! Differential test: random CSV data + random queries, engine vs SQLite (the oracle).
//!
//! * `cargo test`                                   small data, a few seeds (fast)
//! * `cargo test --release --test difftest -- --ignored`  larger data / more queries
//!   env: DIFFTEST_SEED (default 1000), DIFFTEST_QUERIES (default 1500), DIFFTEST_ROWS (default 20000)
//!
//! The same in-memory rows are written to CSV (for the engine, through its real CSV parser) and
//! inserted into SQLite, so the test needs no CSV parser of its own. Comparison rules (CLAUDE.md):
//! NULL = NULL, unordered results compare as multisets, numbers with relative tolerance 1e-9
//! (integers exactly when both sides are integers).
mod common;
use common::Dribble;
use engine::types::{fmt_date, fmt_dec, Value};
use rusqlite::{types::Value as SqlVal, types::ValueRef, Connection};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

// ----------------------------------------------------------------------------- rng

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + self.below((hi - lo + 1) as u64) as i64
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.below(100) < pct
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len() as u64) as usize]
    }
}

// ----------------------------------------------------------------------------- data

#[derive(Clone, Debug)]
enum Cell {
    Null,
    I(i64),
    F(f64),
    D(i64, u8),
    S(String),
    Date(i32),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum K {
    I,
    D,
    F,
    S,
    Dt,
}

struct Table {
    name: &'static str,
    cols: Vec<(&'static str, K, &'static str /* schema.toml type */)>,
    rows: Vec<Vec<Cell>>,
}

const G_POOL: &[&str] = &["red", "green", "blue", "a,b", "say \"hi\"", "line\nbreak", "ünï", "x", "o'neil", "  pad ", "CR\r\nLF"];
const G_CLEAN: &[&str] = &["red", "green", "blue", "ab", "say hi", "x", "oneil", "  pad ", "zz"];
const LIT_POOL: &[&str] = &["red", "green", "blue", "a,b", "ünï", "x", "o'neil", "zzz", "a"];

fn mn(rng: &mut Rng, pct: u64, f: impl FnOnce(&mut Rng) -> Cell) -> Cell {
    if rng.chance(pct) {
        Cell::Null
    } else {
        f(rng)
    }
}

fn gen_tables(rng: &mut Rng, n: usize, clean: bool) -> Vec<Table> {
    let pool: &[&str] = if clean { G_CLEAN } else { G_POOL };
    let nd = (n / 10).max(8) as i64;
    let date0 = 18262; // 2020-01-01
    let fval = |rng: &mut Rng| -> f64 {
        if rng.chance(70) {
            rng.range(-400, 400) as f64 / 8.0
        } else {
            (rng.next() % 2_000_000) as f64 / 1000.0 - 1000.0
        }
    };
    let mut facts = Table {
        name: "facts",
        cols: vec![
            ("id", K::I, "i64"),
            ("k", K::I, "i64"),
            ("g", K::S, "str"),
            ("v", K::I, "i64"),
            ("p", K::D, "decimal(2)"),
            ("f", K::F, "f64"),
            ("d", K::Dt, "date"),
            ("s", K::S, "str"),
        ],
        rows: vec![],
    };
    for i in 0..n as i64 {
        let k = mn(rng, 8, |rng| Cell::I(rng.range(0, nd + 3)));
        let g = mn(rng, 10, |rng| Cell::S(rng.pick(pool).to_string()));
        let v = mn(rng, 10, |rng| Cell::I(rng.range(-50, 50)));
        let p = mn(rng, 10, |rng| Cell::D(rng.range(-10000, 10000), 2));
        let f = mn(rng, 10, |rng| Cell::F(fval(rng)));
        let d = mn(rng, 10, |rng| Cell::Date(date0 + rng.range(-400, 600) as i32));
        let s = mn(rng, 5, |rng| {
            Cell::S(match if clean { 5 } else { rng.below(6) } {
                0 => format!("w{},{}", rng.below(40), rng.below(5)),
                1 => format!("q\"{}\"", rng.below(40)),
                2 => format!("n\n{}", rng.below(40)),
                _ => format!("w{}", rng.below(60)),
            })
        });
        facts.rows.push(vec![Cell::I(i), k, g, v, p, f, d, s]);
    }
    let mut dim = Table {
        name: "dim",
        cols: vec![("id", K::I, "i64"), ("name", K::S, "str"), ("grp", K::S, "str"), ("w", K::D, "decimal(1)")],
        rows: vec![],
    };
    for i in 0..nd {
        if rng.chance(10) {
            continue; // some facts.k have no dim row
        }
        let id = if rng.chance(3) { Cell::Null } else { Cell::I(i) };
        let grp = mn(rng, 10, |rng| Cell::S(rng.pick(pool).to_string()));
        let w = mn(rng, 10, |rng| Cell::D(rng.range(-200, 200), 1));
        dim.rows.push(vec![id, Cell::S(format!("dim{i}")), grp, w]);
    }
    let mut other = Table {
        name: "other",
        cols: vec![("id", K::I, "i64"), ("tag", K::S, "str"), ("x", K::F, "f64")],
        rows: vec![],
    };
    for _ in 0..(n / 5).max(6) {
        let id = mn(rng, 5, |rng| Cell::I(rng.range(0, nd)));
        let tag = mn(rng, 10, |rng| Cell::S(rng.pick(pool).to_string()));
        let x = mn(rng, 10, |rng| Cell::F(fval(rng)));
        other.rows.push(vec![id, tag, x]);
    }
    vec![facts, dim, other]
}

fn cell_text(c: &Cell) -> String {
    let mut b = Vec::new();
    match c {
        Cell::Null => {}
        Cell::I(x) => b.extend_from_slice(x.to_string().as_bytes()),
        Cell::F(x) => b.extend_from_slice(format!("{x}").as_bytes()),
        Cell::D(x, s) => fmt_dec(*x, *s, &mut b),
        Cell::S(s) => b.extend_from_slice(s.as_bytes()),
        Cell::Date(d) => fmt_date(*d, &mut b),
    }
    String::from_utf8(b).unwrap()
}

fn write_csv(dir: &PathBuf, t: &Table, rng: &mut Rng, clean: bool) {
    let eol = if rng.chance(40) { "\r\n" } else { "\n" };
    let final_nl = rng.chance(60);
    let mut out = String::new();
    out.push_str(&t.cols.iter().map(|c| c.0).collect::<Vec<_>>().join(","));
    for row in &t.rows {
        out.push_str(eol);
        for (j, c) in row.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            let s = cell_text(c);
            let special = s.contains([',', '"', '\n', '\r']);
            if special || (!clean && matches!(c, Cell::S(_)) && rng.chance(10)) {
                out.push('"');
                out.push_str(&s.replace('"', "\"\""));
                out.push('"');
            } else {
                out.push_str(&s);
            }
        }
    }
    if final_nl {
        out.push_str(eol);
    }
    std::fs::write(dir.join(format!("{}.csv", t.name)), out).unwrap();
}

fn write_schema(dir: &PathBuf, tables: &[Table]) {
    let mut s = String::new();
    for t in tables {
        writeln!(s, "[{}]", t.name).unwrap();
        for c in &t.cols {
            writeln!(s, "{} = \"{}\"", c.0, c.2).unwrap();
        }
        s.push('\n');
    }
    std::fs::write(dir.join("schema.toml"), s).unwrap();
}

fn load_sqlite(tables: &[Table]) -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    for t in tables {
        let defs: Vec<String> = t
            .cols
            .iter()
            .map(|c| {
                let ty = match c.1 {
                    K::I => "INTEGER",
                    K::D | K::F => "REAL",
                    K::S | K::Dt => "TEXT",
                };
                format!("{} {}", c.0, ty)
            })
            .collect();
        conn.execute_batch(&format!("CREATE TABLE {}({});", t.name, defs.join(","))).unwrap();
        let ph = vec!["?"; t.cols.len()].join(",");
        let mut st = conn.prepare(&format!("INSERT INTO {} VALUES ({})", t.name, ph)).unwrap();
        for row in &t.rows {
            let vals: Vec<SqlVal> = row
                .iter()
                .map(|c| match c {
                    Cell::Null => SqlVal::Null,
                    Cell::I(x) => SqlVal::Integer(*x),
                    Cell::F(x) => SqlVal::Real(*x),
                    // decimals go in as the REAL nearest the decimal text (the oracle has no exact decimal)
                    Cell::D(..) => SqlVal::Real(cell_text(c).parse().unwrap()),
                    Cell::S(s) => SqlVal::Text(s.clone()),
                    Cell::Date(_) => SqlVal::Text(cell_text(c)),
                })
                .collect();
            st.execute(rusqlite::params_from_iter(vals)).unwrap();
        }
    }
    conn
}

// ----------------------------------------------------------------------------- query generator

#[derive(Clone)]
struct Col {
    tbl: &'static str, // alias, "" for unqualified
    name: &'static str,
    ty: K,
}
impl Col {
    fn r(&self) -> String {
        if self.tbl.is_empty() {
            self.name.to_string()
        } else {
            format!("{}.{}", self.tbl, self.name)
        }
    }
}

fn scope_of(t: &Table, alias: &'static str) -> Vec<Col> {
    t.cols.iter().map(|c| Col { tbl: alias, name: c.0, ty: c.1 }).collect()
}

fn sql_str(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn num_cols(sc: &[Col]) -> Vec<&Col> {
    sc.iter().filter(|c| matches!(c.ty, K::I | K::D | K::F)).collect()
}
fn real_cols(sc: &[Col]) -> Vec<&Col> {
    sc.iter().filter(|c| matches!(c.ty, K::D | K::F)).collect()
}

fn num_lit(rng: &mut Rng) -> String {
    match rng.below(4) {
        0 => rng.range(-9, 9).to_string(),
        1 => rng.range(0, 100).to_string(),
        2 => format!("{}.{}", rng.range(-5, 5), rng.below(100)),
        _ => format!("{}.5", rng.range(0, 30)),
    }
}

fn num_expr(rng: &mut Rng, sc: &[Col], depth: u32) -> String {
    let nc = num_cols(sc);
    if depth == 0 || rng.chance(35) {
        return if !nc.is_empty() && rng.chance(75) { rng.pick(&nc).r() } else { num_lit(rng) };
    }
    // `/` is always floating point in the engine, so only divide real-typed operands
    // (SQLite would do integer division on two integers).
    let rc = real_cols(sc);
    if !rc.is_empty() && rng.chance(15) {
        let b = if rng.chance(50) { rng.pick(&rc).r() } else { "2.5".to_string() };
        return format!("({} / {})", rng.pick(&rc).r(), b);
    }
    let op = *rng.pick(&["+", "-", "*"]);
    let a = num_expr(rng, sc, depth - 1);
    let b = if op == "*" { num_expr(rng, sc, 0) } else { num_expr(rng, sc, depth - 1) };
    format!("({a} {op} {b})")
}

fn cmp_op(rng: &mut Rng) -> &'static str {
    rng.pick(&["=", "<>", "<", "<=", ">", ">="])
}

fn date_lit(rng: &mut Rng) -> String {
    let mut b = Vec::new();
    fmt_date(18262 + rng.range(-300, 500) as i32, &mut b);
    format!("'{}'", String::from_utf8(b).unwrap())
}

fn pred(rng: &mut Rng, sc: &[Col], depth: u32) -> String {
    if depth > 0 && rng.chance(40) {
        let a = pred(rng, sc, depth - 1);
        let b = pred(rng, sc, depth - 1);
        return match rng.below(4) {
            0 | 1 => format!("({a} AND {b})"),
            2 => format!("({a} OR {b})"),
            _ => format!("(NOT {a})"),
        };
    }
    let strs: Vec<&Col> = sc.iter().filter(|c| c.ty == K::S).collect();
    let dates: Vec<&Col> = sc.iter().filter(|c| c.ty == K::Dt).collect();
    match rng.below(7) {
        0 | 1 | 2 if !num_cols(sc).is_empty() => {
            let l = num_expr(rng, sc, 2);
            let r = if rng.chance(50) { num_lit(rng) } else { num_expr(rng, sc, 1) };
            // The oracle keeps decimals as REAL, so derived values (x*y, x/y, sums) carry float error
            // that the engine's exact decimals don't. Equality/boundary tests on them would be oracle
            // artifacts; only plain column/literal operands get =, <>, <=, >=.
            let plain = !l.starts_with('(') && !r.starts_with('(');
            if plain {
                format!("{l} {} {r}", cmp_op(rng))
            } else {
                // compare derived values only against an off-grid literal
                format!("{l} {} {}.037191", *rng.pick(&["<", ">"]), rng.range(-9, 9))
            }
        }
        3 if !strs.is_empty() => {
            let c = rng.pick(&strs).r();
            if rng.chance(25) && strs.len() > 1 {
                format!("{c} {} {}", cmp_op(rng), rng.pick(&strs).r())
            } else {
                format!("{c} {} {}", cmp_op(rng), sql_str(rng.pick(LIT_POOL)))
            }
        }
        4 if !dates.is_empty() => format!("{} {} {}", rng.pick(&dates).r(), cmp_op(rng), date_lit(rng)),
        _ => {
            let c = rng.pick(sc);
            format!("{} IS {}NULL", c.r(), if rng.chance(50) { "NOT " } else { "" })
        }
    }
}

fn agg(rng: &mut Rng, sc: &[Col]) -> String {
    let nc = num_cols(sc);
    match rng.below(9) {
        0 => "COUNT(*)".into(),
        1 => format!("COUNT({})", rng.pick(sc).r()),
        2 => format!("COUNT(DISTINCT {})", rng.pick(sc).r()),
        3 | 4 if !nc.is_empty() => format!("SUM({})", num_expr(rng, sc, 2)),
        5 if !nc.is_empty() => format!("AVG({})", num_expr(rng, sc, 2)),
        6 => format!("MIN({})", rng.pick(sc).r()),
        7 => format!("MAX({})", rng.pick(sc).r()),
        _ if !nc.is_empty() => format!("{}({})", rng.pick(&["MIN", "MAX"]), num_expr(rng, sc, 1)),
        _ => "COUNT(*)".into(),
    }
}

/// Build SELECT ... over `from` with columns in `sc`. Returns (sql, ordered).
fn build(rng: &mut Rng, from: &str, sc: &[Col], mut conds: Vec<String>) -> (String, bool) {
    if rng.chance(55) {
        conds.push(pred(rng, sc, 2));
    }
    let where_ = if conds.is_empty() { String::new() } else { format!(" WHERE {}", conds.join(" AND ")) };
    let mode = rng.below(10);
    if mode < 3 {
        // plain projection: plain columns first, computed expressions last (see ordering note below)
        let mut items: Vec<String> = Vec::new();
        let ordered = rng.chance(60);
        if ordered {
            // lead with identifying columns so the ORDER BY is effectively total
            for c in sc.iter().filter(|c| c.name == "id" || c.name == "name") {
                items.push(c.r());
            }
        }
        for _ in 0..rng.range(1, 3) {
            let c = rng.pick(sc).r();
            if !items.contains(&c) {
                items.push(c);
            }
        }
        if rng.chance(50) && !num_cols(sc).is_empty() {
            items.push(num_expr(rng, sc, 2));
        }
        let mut sql = format!("SELECT {} FROM {from}{where_}", items.join(", "));
        if ordered {
            let ob: Vec<String> = (1..=items.len()).map(|i| format!("{i}{}", if rng.chance(30) { " DESC" } else { "" })).collect();
            write!(sql, " ORDER BY {}", ob.join(", ")).unwrap();
            if rng.chance(30) {
                write!(sql, " LIMIT {}", rng.range(0, 40)).unwrap();
            }
        }
        (sql, ordered)
    } else if mode < 5 {
        // global aggregate
        let aggs: Vec<String> = (0..rng.range(1, 4)).map(|_| agg(rng, sc)).collect();
        let mut sql = format!("SELECT {} FROM {from}{where_}", aggs.join(", "));
        if rng.chance(20) {
            write!(sql, " HAVING COUNT(*) {} {}", cmp_op(rng), rng.range(0, 30)).unwrap();
        }
        (sql, false)
    } else {
        let keyable: Vec<&Col> = sc.iter().filter(|c| c.ty != K::F).collect();
        let mut keys: Vec<String> = Vec::new();
        for _ in 0..rng.range(1, 2) {
            let k = rng.pick(&keyable).r();
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        let aggs: Vec<String> = (0..rng.range(1, 3)).map(|_| agg(rng, sc)).collect();
        let mut items = keys.clone();
        items.extend(aggs.iter().cloned());
        if rng.chance(25) {
            // expression over a key and an aggregate
            items.push(format!("COUNT(*) + 1"));
        }
        let mut sql = format!("SELECT {} FROM {from}{where_} GROUP BY {}", items.join(", "), keys.join(", "));
        if rng.chance(30) {
            let h = match rng.below(3) {
                0 => format!("COUNT(*) {} {}", cmp_op(rng), rng.range(0, 20)),
                1 => {
                    let a = if !num_cols(sc).is_empty() && rng.chance(60) {
                        format!("{}({})", rng.pick(&["SUM", "AVG"]), num_expr(rng, sc, 1))
                    } else {
                        format!("COUNT({})", rng.pick(sc).r())
                    };
                    // off-grid literal: a decimal SUM/AVG can equal an integer exactly, a float one not
                    format!("{a} {} {}.037191", *rng.pick(&["<", ">"]), rng.range(-20, 20))
                }
                _ => format!("{} IS NOT NULL", keys[0]),
            };
            write!(sql, " HAVING {h}").unwrap();
        }
        let ordered = rng.chance(65);
        if ordered {
            // keys first -> total order on groups; the remaining positions only break non-existent ties
            let ob: Vec<String> = (1..=items.len()).map(|i| format!("{i}{}", if rng.chance(30) { " DESC" } else { "" })).collect();
            write!(sql, " ORDER BY {}", ob.join(", ")).unwrap();
            if rng.chance(30) {
                write!(sql, " LIMIT {}", rng.range(0, 25)).unwrap();
            }
        }
        (sql, ordered)
    }
}

fn gen_query(rng: &mut Rng, tabs: &[Table]) -> (String, bool) {
    let (f, d, o) = (&tabs[0], &tabs[1], &tabs[2]);
    let jk = |rng: &mut Rng| if rng.chance(70) { "JOIN" } else { "LEFT JOIN" };
    match rng.below(11) {
        0 | 1 => {
            let sc = scope_of(f, if rng.chance(50) { "f" } else { "" });
            let from = if sc[0].tbl.is_empty() { "facts".to_string() } else { "facts f".to_string() };
            build(rng, &from, &sc, vec![])
        }
        2 => {
            let sc = scope_of(d, "");
            build(rng, "dim", &sc, vec![])
        }
        3 | 4 | 5 => {
            // facts x dim, a variety of key shapes (int/int, reversed, two keys, int/int via v, decimal scale 2 vs 1)
            let on = match rng.below(5) {
                0 | 1 => "f.k = d.id".to_string(),
                2 => "d.id = f.k".to_string(),
                3 => "f.k = d.id AND f.g = d.grp".to_string(),
                _ => (*rng.pick(&["f.v = d.id", "f.p = d.w"])).to_string(),
            };
            let mut sc = scope_of(f, "f");
            sc.extend(scope_of(d, "d"));
            if rng.chance(25) {
                // dim on the left, facts joined in (left side smaller -> exercises build-side choice)
                let mut sc2 = scope_of(d, "d");
                sc2.extend(scope_of(f, "f"));
                { let j = jk(rng); build(rng, &format!("dim d {j} facts f ON {on}"), &sc2, vec![]) }
            } else {
                { let j = jk(rng); build(rng, &format!("facts f {j} dim d ON {on}"), &sc, vec![]) }
            }
        }
        6 => {
            let mut sc = scope_of(f, "f");
            sc.extend(scope_of(d, "d"));
            sc.extend(scope_of(o, "o"));
            let on2 = *rng.pick(&["d.id = o.id", "f.v = o.id", "o.id = f.k", "f.f = o.x"]);
            let (j1, j2) = (jk(rng), jk(rng));
            let from = format!("facts f {j1} dim d ON f.k = d.id {j2} other o ON {on2}");
            build(rng, &from, &sc, vec![])
        }
        7 | 8 => {
            // comma join; the equality lives in WHERE
            let mut sc = scope_of(f, "f");
            sc.extend(scope_of(d, "d"));
            build(rng, "facts f, dim d", &sc, vec!["f.k = d.id".into()])
        }
        9 => {
            // many-to-many self join on a string key
            let mut sc = scope_of(d, "a");
            sc.extend(scope_of(d, "b"));
            { let j = jk(rng); build(rng, &format!("dim a {j} dim b ON a.grp = b.grp"), &sc, vec![]) }
        }
        _ => {
            let mut sc = scope_of(o, "o");
            sc.extend(scope_of(f, "f"));
            { let j = jk(rng); build(rng, &format!("other o {j} facts f ON o.x = f.f"), &sc, vec![]) }
        }
    }
}

// ----------------------------------------------------------------------------- comparison

#[derive(Clone, Debug)]
enum V {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
}

fn from_engine(v: &Value) -> V {
    match v {
        Value::Null => V::Null,
        Value::Bool(b) => V::Int(*b as i64),
        Value::I64(x) => V::Int(*x),
        Value::F64(x) => V::Real(*x),
        Value::Dec(x, s) => V::Real(*x as f64 / 10f64.powi(*s as i32)),
        Value::Str(s) => V::Text(s.clone()),
        Value::Date(d) => {
            let mut b = Vec::new();
            fmt_date(*d, &mut b);
            V::Text(String::from_utf8(b).unwrap())
        }
    }
}

fn num_of(v: &V) -> Option<f64> {
    match v {
        V::Int(x) => Some(*x as f64),
        V::Real(x) => Some(*x),
        _ => None,
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

fn cell_eq(a: &V, b: &V) -> bool {
    match (a, b) {
        (V::Null, V::Null) => true,
        (V::Int(x), V::Int(y)) => x == y,
        (V::Text(x), V::Text(y)) => x == y,
        _ => match (num_of(a), num_of(b)) {
            (Some(x), Some(y)) => close(x, y),
            _ => false,
        },
    }
}

fn row_eq(a: &[V], b: &[V]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| cell_eq(x, y))
}

/// Rows can only match if their non-numeric cells match exactly; bucket on those.
fn bucket_key(r: &[V]) -> String {
    let mut k = String::new();
    for c in r {
        match c {
            V::Null => k.push_str("\u{1}N"),
            V::Text(s) => {
                k.push_str("\u{1}T");
                k.push_str(s)
            }
            _ => k.push_str("\u{1}#"),
        }
    }
    k
}

fn num_key(r: &[V]) -> Vec<f64> {
    r.iter().filter_map(num_of).collect()
}

fn multiset_eq(a: &[Vec<V>], b: &[Vec<V>]) -> Result<(), String> {
    let mut ba: BTreeMap<String, Vec<&Vec<V>>> = BTreeMap::new();
    let mut bb: BTreeMap<String, Vec<&Vec<V>>> = BTreeMap::new();
    for r in a {
        ba.entry(bucket_key(r)).or_default().push(r);
    }
    for r in b {
        bb.entry(bucket_key(r)).or_default().push(r);
    }
    for (k, ra) in ba.iter_mut() {
        let Some(rb) = bb.get_mut(k) else { return Err(format!("engine row {:?} has no counterpart in SQLite", ra[0])) };
        if ra.len() != rb.len() {
            return Err(format!("row group {:?}: engine has {} rows, SQLite has {}", ra[0], ra.len(), rb.len()));
        }
        let cmp = |x: &&Vec<V>, y: &&Vec<V>| num_key(x).partial_cmp(&num_key(y)).unwrap_or(std::cmp::Ordering::Equal);
        ra.sort_by(cmp);
        rb.sort_by(cmp);
        if ra.iter().zip(rb.iter()).all(|(x, y)| row_eq(x, y)) {
            continue;
        }
        // near-equal floats may have sorted differently: fall back to greedy matching
        if ra.len() > 4000 {
            return Err("mismatch in a very large row group".into());
        }
        let mut used = vec![false; rb.len()];
        for x in ra.iter() {
            match (0..rb.len()).find(|&j| !used[j] && row_eq(x, rb[j])) {
                Some(j) => used[j] = true,
                None => return Err(format!("engine row {x:?} has no matching SQLite row")),
            }
        }
    }
    for k in bb.keys() {
        if !ba.contains_key(k) {
            return Err(format!("SQLite row {:?} has no counterpart in engine", bb[k][0]));
        }
    }
    Ok(())
}

fn compare(engine: &[Vec<V>], oracle: &[Vec<V>], ordered: bool) -> Result<(), String> {
    if engine.len() != oracle.len() {
        return Err(format!("row count: engine {} vs SQLite {}", engine.len(), oracle.len()));
    }
    if ordered {
        for (i, (a, b)) in engine.iter().zip(oracle).enumerate() {
            if !row_eq(a, b) {
                return Err(format!("row {i} differs:\n  engine: {a:?}\n  sqlite: {b:?}"));
            }
        }
        Ok(())
    } else {
        multiset_eq(engine, oracle)
    }
}

fn sqlite_rows(conn: &Connection, sql: &str) -> Result<Vec<Vec<V>>, String> {
    let mut st = conn.prepare(sql).map_err(|e| format!("sqlite prepare: {e}"))?;
    let n = st.column_count();
    let mut rows = st.query([]).map_err(|e| format!("sqlite query: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().map_err(|e| format!("sqlite step: {e}"))? {
        out.push(
            (0..n)
                .map(|i| match r.get_ref(i).unwrap() {
                    ValueRef::Null => V::Null,
                    ValueRef::Integer(x) => V::Int(x),
                    ValueRef::Real(x) => V::Real(x),
                    ValueRef::Text(t) => V::Text(String::from_utf8_lossy(t).into_owned()),
                    ValueRef::Blob(_) => V::Null,
                })
                .collect(),
        );
    }
    Ok(out)
}

// ----------------------------------------------------------------------------- driver

fn run(seed: u64, rows: usize, queries: usize, name: &str) {
    let mut rng = Rng::new(seed);
    // odd seeds produce quote-free files (exercises the quote-free scan fast path), even seeds the full zoo
    let clean = seed % 2 == 1;
    let tabs = gen_tables(&mut rng, rows, clean);
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("difftest-data").join(format!("{name}-{seed}"));
    std::fs::create_dir_all(&dir).unwrap();
    for t in &tabs {
        write_csv(&dir, t, &mut rng, clean);
    }
    write_schema(&dir, &tabs);
    let conn = load_sqlite(&tabs);

    let mut failures = Vec::new();
    let mut nonempty = 0;
    let (mut stream_ran, mut stream_skipped) = (0usize, 0usize);
    for qi in 0..queries {
        let (sql, ordered) = gen_query(&mut rng, &tabs);
        let want = match sqlite_rows(&conn, &sql) {
            Ok(r) => r,
            Err(e) => panic!("generator produced SQL SQLite rejects (test bug): {e}\n  {sql}"),
        };
        // both the default (optimized) path and the --no-opt reference path must match the oracle
        // default = parallel path with 16 MiB chunks (one chunk here); stress = 3 threads and 256-byte
        // chunks, so splits land inside quoted fields, between CRLF pairs, etc.
        let reference = engine::run_collect_opts(&dir, &sql, &engine::Options { no_opt: true, ..Default::default() });
        let fast = engine::run_collect_opts(&dir, &sql, &engine::Options::default());
        let stress = engine::run_collect_opts(
            &dir,
            &sql,
            &engine::Options { threads: Some(3), chunk_bytes: Some(256), agg_partition_groups: Some(0), ..Default::default() },
        );
        // stream configuration: the FROM table is read as a stream (random 1..=300-byte reads, 37-byte blocks)
        // in both the default and the --no-opt engine. Queries that hit a documented streaming limit are
        // skipped and counted.
        let ftable = sql.split(" FROM ").nth(1).and_then(|r| r.split_whitespace().next()).unwrap().to_string();
        let mut stream_runs = Vec::new();
        for (label, no_opt) in [("stream/37B blocks", false), ("stream/--no-opt", true)] {
            let csv = std::fs::read(dir.join(format!("{ftable}.csv"))).unwrap();
            let o = engine::Options { chunk_bytes: Some(37), no_opt, ..Default::default() };
            let r = engine::run_collect_stream(
                &dir,
                &sql,
                &o,
                &ftable,
                Box::new(Dribble::new(&csv, seed * 1000 + qi as u64)),
            );
            match r {
                Err(e) if e.0.contains("cannot be the right side of a LEFT JOIN") || e.0.contains("can be scanned only once") => {
                    stream_skipped += 1
                }
                r => {
                    stream_ran += 1;
                    stream_runs.push((label, r))
                }
            }
        }
        for (label, res) in [("default", fast), ("--no-opt", reference), ("3 threads/256B chunks/forced partitioned merge", stress)]
            .into_iter()
            .chain(stream_runs)
        {
          match res {
            Err(e) => failures.push(format!("[seed {seed} q{qi}] {label} engine error: {e}\n  {sql}")),
            Ok((_, got)) => {
                let got: Vec<Vec<V>> = got.iter().map(|r| r.iter().map(from_engine).collect()).collect();
                if !want.is_empty() {
                    nonempty += 1;
                }
                if let Err(m) = compare(&got, &want, ordered) {
                    failures.push(format!("[seed {seed} q{qi}] MISMATCH {label} ({}): {m}\n  {sql}", if ordered { "ordered" } else { "multiset" }));
                }
            }
          }
        }
        if failures.len() >= 5 {
            break;
        }
    }
    assert!(failures.is_empty(), "\n{}\n(data dir: {})", failures.join("\n\n"), dir.display());
    eprintln!(
        "difftest seed {seed}: {queries} queries OK ({nonempty} with non-empty results; stream runs: {stream_ran} compared, {stream_skipped} skipped for documented streaming limits)"
    );
}

#[test]
fn difftest_small() {
    for seed in 1..=6 {
        run(seed, 300, 250, "small");
    }
}

#[test]
#[ignore]
fn difftest_large() {
    let env = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let seed = env("DIFFTEST_SEED", 1000);
    let rows = env("DIFFTEST_ROWS", 20_000) as usize;
    let queries = env("DIFFTEST_QUERIES", 1500) as usize;
    for s in 0..3 {
        run(seed + s, rows, queries, "large");
    }
}

// ----------------------------------------------------------------------------- EXP-008: join key cases
//
// Directed join cases against SQLite. The integer-key join (`ON f.k = d.k`, also date and decimal keys) and the
// byte-key fallback (string key, composite key) are run on the SAME data and must both equal SQLite. Thread-local
// counters from the engine prove which index each run really used.

const JDATE0: i32 = 18262; // 2020-01-01

fn join_row_f(id: i64, k: Option<i64>, rng: &mut Rng) -> Vec<Cell> {
    let v = Cell::I(rng.range(-9, 9));
    match k {
        Some(k) => vec![
            Cell::I(id),
            Cell::I(k),
            Cell::S(format!("s{k}")),
            Cell::I(k.rem_euclid(3)),
            Cell::Date(JDATE0 + k.rem_euclid(50) as i32),
            Cell::D(k.rem_euclid(97) * 25, 2),
            v,
        ],
        None => vec![Cell::I(id), Cell::Null, Cell::Null, Cell::Null, Cell::Null, Cell::Null, v],
    }
}

fn join_row_d(k: Option<i64>, rng: &mut Rng) -> Vec<Cell> {
    let w = Cell::I(rng.range(1, 100));
    match k {
        Some(k) => vec![
            Cell::I(k),
            Cell::S(format!("s{k}")),
            Cell::I(k.rem_euclid(3)),
            Cell::Date(JDATE0 + k.rem_euclid(50) as i32),
            Cell::D(k.rem_euclid(97) * 25, 2),
            w,
        ],
        None => vec![Cell::Null, Cell::Null, Cell::Null, Cell::Null, Cell::Null, w],
    }
}

fn join_tables(f_keys: Vec<Option<i64>>, d_keys: Vec<Option<i64>>, rng: &mut Rng) -> Vec<Table> {
    let f = Table {
        name: "f",
        cols: vec![
            ("id", K::I, "i64"),
            ("k", K::I, "i64"),
            ("ks", K::S, "str"),
            ("k2", K::I, "i64"),
            ("dt", K::Dt, "date"),
            ("dc", K::D, "decimal(2)"),
            ("v", K::I, "i64"),
        ],
        rows: f_keys.into_iter().enumerate().map(|(i, k)| join_row_f(i as i64, k, rng)).collect(),
    };
    let d = Table {
        name: "d",
        cols: vec![
            ("k", K::I, "i64"),
            ("ks", K::S, "str"),
            ("k2", K::I, "i64"),
            ("dt", K::Dt, "date"),
            ("dc", K::D, "decimal(2)"),
            ("w", K::I, "i64"),
        ],
        rows: d_keys.into_iter().map(|k| join_row_d(k, rng)).collect(),
    };
    vec![f, d]
}

/// (name, f keys, d keys, big?) - every variant is run through both index kinds.
fn join_variants(rng: &mut Rng) -> Vec<(&'static str, Vec<Table>, bool)> {
    let some = |v: Vec<i64>| v.into_iter().map(Some).collect::<Vec<_>>();
    let mut out = Vec::new();
    // sparse keys: only the high bits differ (j << 40), plus keys near the i64 limits
    let sparse_pool: Vec<i64> = (1..=40i64)
        .map(|j| j << 40)
        .chain([0, 4_000_000_000_000_000_000, -4_000_000_000_000_000_000, 1 << 62, -(1 << 62), i64::MAX - 1, i64::MIN + 1])
        .collect();
    let fk = some((0..300).map(|_| *rng.pick(&sparse_pool)).collect());
    let dk = some(sparse_pool.clone());
    out.push(("sparse", join_tables(fk, dk, rng), false));
    // negative keys
    let fk = some((0..300).map(|_| rng.range(-320, 320)).collect());
    let dk = some((-300..=300).collect());
    out.push(("negative", join_tables(fk, dk, rng), false));
    // NULL keys on both sides (NULL never matches NULL)
    let nullable = |rng: &mut Rng, n: usize, hi: i64| -> Vec<Option<i64>> {
        (0..n).map(|_| if rng.chance(25) { None } else { Some(rng.range(0, hi)) }).collect()
    };
    let (fk, dk) = (nullable(rng, 300, 30), nullable(rng, 50, 30));
    out.push(("nulls", join_tables(fk, dk, rng), false));
    // duplicate build keys (1 to 5 copies each), with the dimension both as the right and the left side
    let fk = some((0..300).map(|_| rng.range(0, 45)).collect());
    let mut dk = Vec::new();
    for k in 0..40 {
        for _ in 0..rng.range(1, 5) {
            dk.push(Some(k));
        }
    }
    out.push(("duplicates", join_tables(fk, dk, rng), false));
    // unmatched probe rows and NULLs for LEFT JOIN
    let (fk, dk) = (nullable(rng, 300, 60), nullable(rng, 30, 25));
    out.push(("unmatched", join_tables(fk, dk, rng), false));
    // empty tables
    out.push(("empty-d", join_tables(some((0..40).map(|_| rng.range(0, 9)).collect()), vec![], rng), false));
    out.push(("empty-f", join_tables(vec![], some((0..30).collect()), rng), false));
    out.push(("empty-both", join_tables(vec![], vec![], rng), false));
    // large build side: at least 65,536 rows, so the partitioned parallel build runs; dense, sparse, negative, NULL, duplicates
    let big_key = |rng: &mut Rng| -> Option<i64> {
        match rng.below(100) {
            0..=2 => None,
            3..=29 => Some(rng.range(-20_000, 20_000)),
            30..=49 => Some(rng.range(0, 3_000) << 36),
            _ => Some(rng.range(0, 40_000)),
        }
    };
    let dk: Vec<Option<i64>> = (0..70_000).map(|_| big_key(rng)).collect();
    let fk: Vec<Option<i64>> = (0..20_000).map(|_| big_key(rng)).collect();
    out.push(("big-70k", join_tables(fk, dk, rng), true));
    out
}

#[test]
fn difftest_join_key_cases() {
    use engine::exec::join_stats;
    let mut rng = Rng::new(808);
    let mut failures = Vec::new();
    let (mut compared, mut nonempty) = (0usize, 0usize);
    for (name, tabs, big) in join_variants(&mut rng) {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("difftest-data").join(format!("join-{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        for t in &tabs {
            write_csv(&dir, t, &mut rng, true);
        }
        write_schema(&dir, &tabs);
        let conn = load_sqlite(&tabs);
        // (ON clause, runs on the integer path?)
        let mut ons: Vec<(&str, bool)> = vec![("f.k = d.k", true), ("f.ks = d.ks", false), ("f.k = d.k AND f.k2 = d.k2", false)];
        if !big {
            ons.push(("f.dt = d.dt", true));
            ons.push(("f.dc = d.dc", true));
        }
        let shapes: Vec<&str> = if big {
            vec![
                "SELECT COUNT(*), SUM(f.v), SUM(d.w) FROM f JOIN d ON {on}",
                "SELECT COUNT(*), COUNT(d.w), SUM(d.w) FROM f LEFT JOIN d ON {on}",
            ]
        } else {
            vec![
                "SELECT COUNT(*), SUM(f.v), SUM(d.w) FROM f JOIN d ON {on}",
                "SELECT COUNT(*), SUM(f.v), SUM(d.w) FROM d JOIN f ON {on}",
                "SELECT f.id, d.w FROM f JOIN d ON {on}",
                "SELECT d.w, f.id FROM d JOIN f ON {on}",
                "SELECT f.id, d.w FROM f LEFT JOIN d ON {on}",
                "SELECT COUNT(*), COUNT(d.w), SUM(d.w) FROM f LEFT JOIN d ON {on}",
            ]
        };
        let chunk = if big { 64 * 1024 } else { 200 };
        let mut configs: Vec<(String, engine::Options)> = vec![("default".into(), engine::Options::default())];
        for t in [1, 2, 4] {
            configs.push((format!("{t} threads/{chunk}B chunks"), engine::Options { threads: Some(t), chunk_bytes: Some(chunk), ..Default::default() }));
        }
        configs.push(("--no-opt".into(), engine::Options { no_opt: true, ..Default::default() }));
        for shape in &shapes {
            for (on, int_path) in &ons {
                let sql = shape.replace("{on}", on);
                let want = sqlite_rows(&conn, &sql).unwrap_or_else(|e| panic!("SQLite rejects {sql}: {e}"));
                for (label, o) in &configs {
                    let (i0, b0) = (join_stats::int_builds(), join_stats::byte_builds());
                    let res = engine::run_collect_opts(&dir, &sql, o);
                    let (di, db) = (join_stats::int_builds() - i0, join_stats::byte_builds() - b0);
                    let expect_int = *int_path && !o.no_opt;
                    if (expect_int && (di == 0 || db != 0)) || (!expect_int && (di != 0 || db == 0)) {
                        failures.push(format!("[{name}] {label}: wrong join index (int builds {di}, byte builds {db}, expected int path = {expect_int})\n  {sql}"));
                    }
                    match res {
                        Err(e) => failures.push(format!("[{name}] {label} engine error: {e}\n  {sql}")),
                        Ok((_, got)) => {
                            compared += 1;
                            if !want.is_empty() {
                                nonempty += 1;
                            }
                            let got: Vec<Vec<V>> = got.iter().map(|r| r.iter().map(from_engine).collect()).collect();
                            if let Err(m) = compare(&got, &want, false) {
                                failures.push(format!("[{name}] MISMATCH {label}: {m}\n  {sql}"));
                            }
                        }
                    }
                }
                if failures.len() >= 5 {
                    break;
                }
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
    eprintln!("join key cases: {compared} engine runs equal to SQLite ({nonempty} with non-empty results)");
}
