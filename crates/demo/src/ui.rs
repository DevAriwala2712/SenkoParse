//! Frame rendering: pure functions from model to lines of text (ANSI colour optional). Every line is
//! padded to the terminal width so a redraw overwrites the previous frame completely (no flicker, no ghosts).
use crate::model::*;

pub struct View<'a> {
    pub qe: &'a Eng,
    pub duck: &'a Eng,
    pub meta: &'a Meta,
    pub now_ms: u64,
    pub color: bool,
    /// Summary-only replay: recorded numbers are revealed when the clock reaches the recorded wall time.
    pub reveal_clock: bool,
}

const DIM: &str = "38;5;244";
const TEXT: &str = "38;5;252";
const OK: &str = "38;5;78";
const BAD: &str = "38;5;203";
const WARN: &str = "38;5;214";

fn c(v: &View, code: &str, s: &str) -> String {
    if v.color { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() }
}
fn bold(v: &View, code: &str, s: &str) -> String {
    if v.color { format!("\x1b[1;{code}m{s}\x1b[0m") } else { s.to_string() }
}

/// Visible width (escape sequences count as zero).
pub fn vlen(s: &str) -> usize {
    let (mut n, mut esc) = (0, false);
    for ch in s.chars() {
        if esc {
            if ch.is_ascii_alphabetic() {
                esc = false;
            }
        } else if ch == '\x1b' {
            esc = true;
        } else {
            n += 1;
        }
    }
    n
}

/// Pad with spaces or cut to exactly `w` visible columns.
pub fn fit(s: &str, w: usize) -> String {
    let l = vlen(s);
    if l <= w {
        return format!("{s}{}", " ".repeat(w - l));
    }
    let (mut out, mut n, mut esc) = (String::new(), 0, false);
    for ch in s.chars() {
        if esc {
            out.push(ch);
            esc = !ch.is_ascii_alphabetic();
        } else if ch == '\x1b' {
            esc = true;
            out.push(ch);
        } else if n < w {
            out.push(ch);
            n += 1;
        }
    }
    if out.contains('\x1b') {
        out.push_str("\x1b[0m");
    }
    out
}

fn center(s: &str, w: usize) -> String {
    let l = vlen(s);
    let left = w.saturating_sub(l) / 2;
    fit(&format!("{}{s}", " ".repeat(left)), w)
}

pub fn gb(b: u64) -> String {
    if b >= 1_000_000_000 {
        format!("{:.2} GB", b as f64 / 1e9)
    } else if b >= 1_000_000 {
        format!("{:.0} MB", b as f64 / 1e6)
    } else {
        format!("{b} B")
    }
}
pub fn mib(kib: u64) -> String {
    format!("{:.0} MiB", kib as f64 / 1024.0)
}
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut o = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            o.push(',');
        }
        o.push(ch);
    }
    o
}
fn secs(ms: u64) -> String {
    format!("{:.1} s", ms as f64 / 1000.0)
}

/// Max RSS per plot column, x = input consumed (shared axis); gaps between measured columns are carried forward.
fn plot_cols(samples: &[Sample], total: u64, w: usize) -> Vec<Option<u64>> {
    let mut cols = vec![None; w];
    if total == 0 || w == 0 {
        return cols;
    }
    for s in samples {
        let col = ((s.bytes as f64 / total as f64) * w as f64) as usize;
        let col = col.min(w - 1);
        cols[col] = Some(cols[col].unwrap_or(0).max(s.rss_kib));
    }
    let last = cols.iter().rposition(|x| x.is_some());
    let mut prev = None;
    for i in 0..w {
        match (cols[i], prev, last) {
            (Some(v), _, _) => prev = Some(v),
            (None, Some(p), Some(l)) if i < l => cols[i] = Some(p),
            _ => {}
        }
    }
    cols
}

const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const ROWS: usize = 3;

fn plot_rows(cols: &[Option<u64>], max_kib: u64) -> Vec<String> {
    (0..ROWS)
        .map(|r| {
            cols.iter()
                .map(|v| match v {
                    None => ' ',
                    Some(k) => {
                        let level = ((*k as f64 / max_kib as f64) * (ROWS * 8) as f64).ceil() as usize;
                        let level = if *k > 0 { level.max(1) } else { 0 };
                        BLOCKS[level.saturating_sub((ROWS - 1 - r) * 8).min(8)]
                    }
                })
                .collect()
        })
        .collect()
}

fn bar(frac: f64, w: usize) -> (String, String) {
    let eighths = (frac.clamp(0.0, 1.0) * (w * 8) as f64).round() as usize;
    let (full, part) = (eighths / 8, eighths % 8);
    let mut on = "█".repeat(full);
    if part > 0 {
        on.push(['▏', '▎', '▍', '▌', '▋', '▊', '▉'][part - 1]);
    }
    let off = "░".repeat(w - full - (part > 0) as usize);
    (on, off)
}

fn rss_scale(v: &View) -> u64 {
    let mut m = 64 * 1024;
    for e in [v.qe, v.duck] {
        m = m.max(e.peak_rss_kib());
        if let Source::Summary(r) = &e.source {
            m = m.max((r.rss_mib * 1024.0) as u64);
        }
    }
    m.div_ceil(50 * 1024) * 50 * 1024
}

fn temp_scale(v: &View) -> u64 {
    let mut m = v.meta.total_bytes * 3 / 2;
    for e in [v.qe, v.duck] {
        m = m.max(e.peak_temp());
        if let Source::Summary(r) = &e.source {
            m = m.max((r.temp_mib * 1048576.0) as u64);
        }
    }
    m.max(1)
}

fn panel(v: &View, e: &Eng, pw: usize) -> Vec<String> {
    let iw = pw - 2;
    let acc = e.accent;
    let side = |s: String| format!("{}{}{}", c(v, DIM, "│"), fit(&s, iw), c(v, DIM, "│"));
    let live = matches!(e.source, Source::Live | Source::Replay);
    let summary = if let Source::Summary(r) = &e.source { Some(r) } else { None };
    let hidden = v.reveal_clock && summary.is_some_and(|r| v.now_ms < (r.wall_s * 1000.0) as u64);
    let last = e.last();
    // title bar
    let status = match (&e.source, &e.end) {
        (Source::Missing(_), _) => c(v, DIM, "not available"),
        (Source::Summary(r), _) if !hidden => c(v, WARN, &format!("recorded · {:.1} s", r.wall_s)),
        (Source::Summary(_), _) => c(v, WARN, "replaying…"),
        (_, Some(end)) => c(v, OK, &format!("✔ done {}", secs(end.t_ms))),
        _ => c(v, acc, &format!("● running {}", secs(v.now_ms))),
    };
    let title = format!(" {} ", bold(v, acc, e.label));
    let fill = pw.saturating_sub(vlen(&title) + vlen(&status) + 6);
    let top = format!("{}{}{}{}{}{}", c(v, DIM, "╭─"), title, c(v, DIM, &"─".repeat(fill)), " ", status, c(v, DIM, " ─╮"));
    let mut out = vec![fit(&top, pw)];
    let pwid = iw.saturating_sub(2);
    let max_kib = rss_scale(v);
    let total = v.meta.total_bytes;
    if let Source::Missing(why) = &e.source {
        out.push(side(format!(" {}", c(v, DIM, why))));
        for _ in 0..7 {
            out.push(side(String::new()));
        }
    } else if let Some(r) = summary {
        let rec = |s: String| c(v, WARN, &s);
        out.push(side(format!(" {}", c(v, TEXT, "RECORDED, not live (EXP-005-stream)"))));
        let shown = !hidden;
        for i in 0..ROWS {
            let msg = if i == 1 { "no time series for this run" } else { "" };
            out.push(side(format!(" {}", c(v, DIM, &center(msg, pwid)))));
        }
        out.push(side(format!(" {}", c(v, DIM, "(recorded totals only)"))));
        let tf = if shown { r.temp_mib * 1048576.0 / temp_scale(v) as f64 } else { 0.0 };
        let bw = iw.saturating_sub(17);
        let (on, off) = bar(tf, bw);
        let val = if shown { gb((r.temp_mib * 1048576.0) as u64) } else { "—".into() };
        out.push(side(format!(" temp {}{} {:>9}", rec(on), c(v, DIM, &off), val)));
        let info = if shown {
            format!(" peak {:.0} MiB · {} rows (median of {})", r.rss_mib, thousands(r.out_rows), r.runs)
        } else {
            " …".to_string()
        };
        out.push(side(c(v, TEXT, &info)));
        out.push(side(String::new()));
    } else {
        let peak = e.peak_rss_kib();
        out.push(side(format!(
            " {} {} · peak {}",
            c(v, DIM, "RSS"),
            bold(v, acc, &mib(last.rss_kib)),
            c(v, TEXT, &mib(peak))
        )));
        let cols = plot_cols(&e.samples, total, pwid);
        for l in plot_rows(&cols, max_kib) {
            out.push(side(format!(" {}", c(v, acc, &l))));
        }
        let total_s = format!("{:.2} GB", total as f64 / 1e9);
        let label = " input GB ";
        let dots = pwid.saturating_sub(2 + total_s.len() + label.len());
        let axis = format!("0{}{}{}{}", "┄".repeat(dots / 2), label, "┄".repeat(dots - dots / 2) + " ", total_s);
        let _ = &axis;
        out.push(side(format!(" {}", c(v, DIM, &axis))));
        let bw = iw.saturating_sub(17);
        let shown_temp = if e.end.is_some() { e.peak_temp() } else { last.temp };
        let (on, off) = bar(shown_temp as f64 / temp_scale(v) as f64, bw);
        out.push(side(format!(" temp {}{} {:>9}", c(v, acc, &on), c(v, DIM, &off), gb(shown_temp))));
        let pct = if total > 0 { last.bytes as f64 * 100.0 / total as f64 } else { 0.0 };
        out.push(side(format!(
            " {} {} ({:.0}%) · {} rows out",
            c(v, DIM, "in"),
            c(v, TEXT, &gb(last.bytes)),
            pct,
            c(v, TEXT, &thousands(e.end.as_ref().map(|x| x.rows).unwrap_or(last.rows)))
        )));
        let _ = live;
        out.push(side(String::new()));
    }
    out.push(fit(&format!("{}", c(v, DIM, &format!("╰{}╯", "─".repeat(iw)))), pw));
    out
}

fn banner(v: &View, w: usize) -> String {
    let (d, q) = (v.duck, v.qe);
    let (dtemp, tag) = match &d.source {
        Source::Summary(r) if v.reveal_clock && v.now_ms < (r.wall_s * 1000.0) as u64 => (0, " (recorded)"),
        Source::Summary(r) => (((r.temp_mib * 1048576.0) as u64), " (recorded)"),
        Source::Missing(_) => (0, " (n/a)"),
        _ => (if d.done() { d.peak_temp() } else { d.last().temp }, ""),
    };
    let ql = q.last();
    let qrss = match &q.source {
        Source::Summary(r) if v.reveal_clock && v.now_ms < (r.wall_s * 1000.0) as u64 => 0,
        Source::Summary(r) => (r.rss_mib * 1024.0) as u64,
        _ if q.end.is_some() => q.peak_rss_kib(),
        _ => ql.rss_kib,
    };
    let tail: Vec<u64> = q.samples.iter().skip(5).map(|s| s.rss_kib).collect();
    let flat = tail.len() >= 8 && tail.iter().max().unwrap_or(&0) * 10 <= tail.iter().min().unwrap_or(&1).max(&1) * 13;
    let s = format!(
        "{}  ·  {}",
        bold(v, d.accent, &format!("DuckDB{tag}: +{} on disk", gb(dtemp))),
        bold(v, q.accent, &format!("SenkoParse: {}{}", mib(qrss), if flat { " flat" } else { "" }))
    );
    center(&s, w)
}

fn card(v: &View, w: usize) -> Vec<String> {
    let (q, d) = (v.qe, v.duck);
    let cw = w.min(72);
    let pad = (w - cw) / 2;
    let inner = cw - 2;
    let col = |s: &str, x: usize| fit(s, x);
    let (c1, c2, c3) = (30, (inner - 30) / 2, inner - 30 - (inner - 30) / 2);
    let row = |a: String, b: String, cc: String| {
        let line = format!("│{}{}{}│", col(&a, c1), col(&b, c2), col(&cc, c3));
        format!("{}{}", " ".repeat(pad), line)
    };
    let get = |e: &Eng| -> (String, String, String, String, u64, String) {
        match (&e.source, &e.end) {
            (Source::Summary(r), _) => (
                format!("{:.0} MiB*", r.rss_mib),
                format!("{}*", gb((r.temp_mib * 1048576.0) as u64)),
                format!("{:.1} s*", r.wall_s),
                format!("{}*", thousands(r.out_rows)),
                r.out_rows,
                String::new(),
            ),
            (_, Some(x)) => (mib(x.peak_rss_kib), gb(x.peak_temp), secs(x.t_ms), thousands(x.rows), x.rows, x.digest.clone()),
            _ => ("n/a".into(), "n/a".into(), "n/a".into(), "n/a".into(), 0, String::new()),
        }
    };
    let (qa, qb, qc, qd, qr, qh) = get(q);
    let (da, db, dc, dd, dr, dh) = get(d);
    let verdict = if q.end.as_ref().is_some_and(|e| !e.ok) || matches!(&d.source, Source::Live | Source::Replay) && d.end.as_ref().is_some_and(|e| !e.ok) {
        c(v, BAD, "✗ an engine exited with an error")
    } else if matches!(d.source, Source::Missing(_)) {
        c(v, DIM, "– DuckDB not run")
    } else if matches!(d.source, Source::Summary(_)) || matches!(q.source, Source::Summary(_)) {
        if qr == dr { c(v, OK, "✓ row counts equal (hash n/a)") } else { c(v, BAD, "✗ row counts differ") }
    } else if qr == dr && !qh.is_empty() && qh == dh {
        c(v, OK, &format!("✓ same rows · hash {}", &qh[..8.min(qh.len())]))
    } else {
        c(v, BAD, &format!("✗ outputs differ ({} vs {} rows)", thousands(qr), thousands(dr)))
    };
    let title = format!(" RESULT · {} · {} piped ", v.meta.size, gb(v.meta.total_bytes));
    let top = format!("╭─{}{}╮", bold(v, TEXT, &title), "─".repeat(inner.saturating_sub(vlen(&title) + 1)));
    let mut o = vec![format!("{}{}", " ".repeat(pad), c(v, DIM, &top))];
    o.push(row(String::new(), bold(v, q.accent, q.label), bold(v, d.accent, d.label)));
    o.push(row(" peak RSS (sampled 100 ms)".into(), qa, da));
    o.push(row(" peak temp on disk".into(), qb, db));
    o.push(row(" wall time (not a claim)".into(), qc, dc));
    o.push(row(" output rows".into(), qd, dd));
    o.push(format!("{}│{}│", " ".repeat(pad), fit(&format!(" rows identical (sorted hash): {verdict}"), inner)));
    o.push(format!("{}{}", " ".repeat(pad), c(v, DIM, &format!("╰{}╯", "─".repeat(inner)))));
    o
}

pub fn render(v: &View, w: usize) -> Vec<String> {
    let w = w.max(80);
    let badge = if v.meta.mode == "LIVE" { bold(v, OK, " LIVE ") } else { bold(v, WARN, " REPLAY ") };
    let head = format!(
        " {} {} {}",
        bold(v, TEXT, "SenkoParse"),
        c(v, DIM, "· bounded input buffering for this streaming join"),
        badge
    );
    let mut lines = vec![fit(&head, w)];
    let (lw, rw) = ((w - 1) / 2, w - 1 - (w - 1) / 2);
    let (l, r) = (panel(v, v.qe, lw), panel(v, v.duck, rw));
    for (a, b) in l.iter().zip(r.iter()) {
        lines.push(format!("{a} {b}"));
    }
    lines.push(banner(v, w));
    let revealed = |e: &Eng| match &e.source { Source::Summary(r) if v.reveal_clock => v.now_ms >= (r.wall_s * 1000.0) as u64, _ => true };
    let done = v.qe.done() && v.duck.done() && revealed(v.qe) && revealed(v.duck);
    if done {
        lines.extend(card(v, w));
    } else {
        let note = if v.meta.note.is_empty() { "result card appears when both engines finish".to_string() } else { v.meta.note.clone() };
        lines.push(center(&c(v, DIM, &note), w));
        for _ in 0..7 {
            lines.push(" ".repeat(w));
        }
    }
    let foot = |s: String| {
        let s = if s.chars().count() > w { format!("{}…", s.chars().take(w - 1).collect::<String>()) } else { s };
        fit(&c(v, DIM, &s), w)
    };
    lines.push(foot(format!(" machine: {}", v.meta.machine)));
    lines.push(foot(format!(" DuckDB {} · SenkoParse {} · input {} ({})", v.meta.duckdb, v.meta.qe_settings, v.meta.size, gb(v.meta.total_bytes))));
    lines.push(foot(format!(" query: {}", v.meta.query)));
    lines.push(foot(format!(" One query, one data shape (cf. DuckDB #25165): not a general speed claim.{}", if matches!(v.qe.source, Source::Summary(_)) || matches!(v.duck.source, Source::Summary(_)) { " * recorded" } else { "" })));
    lines.into_iter().map(|l| fit(&l, w)).collect()
}

/// One plain status line for non-terminal output.
pub fn status_line(v: &View) -> String {
    let f = |e: &Eng| {
        let s = e.last();
        format!("{}: in {} rss {} temp {} rows {}{}", e.key, gb(s.bytes), mib(s.rss_kib), gb(s.temp), thousands(s.rows), if e.end.is_some() { " [done]" } else { "" })
    };
    format!("[{:>5.1}s] {} | {}", v.now_ms as f64 / 1000.0, f(v.qe), f(v.duck))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_pads_and_cuts_by_visible_width() {
        assert_eq!(vlen("\x1b[1;31mabc\x1b[0m"), 3);
        assert_eq!(vlen(&fit("\x1b[31mabc\x1b[0m", 6)), 6);
        assert_eq!(vlen(&fit("abcdefgh", 4)), 4);
    }

    #[test]
    fn thousands_and_bytes() {
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(gb(1_800_000_000), "1.80 GB");
    }

    /// The committed live recording replays into a frame whose every line is exactly the terminal width.
    #[test]
    fn recorded_run_renders_at_every_width() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/experiments/data/demo-live-5x.jsonl")).unwrap();
        let rec = load_recording(&text).expect("recording");
        let mut qe = Eng::new("qe", "SenkoParse", "38;5;43", Source::Replay);
        let mut duck = Eng::new("duckdb", "DuckDB (piped)", "38;5;214", Source::Replay);
        qe.samples = rec.qe.0.clone();
        qe.end = rec.qe.1.clone();
        duck.samples = rec.duck.0.clone();
        duck.end = rec.duck.1.clone();
        for w in [80usize, 101, 140] {
            let v = View { qe: &qe, duck: &duck, meta: &rec.meta, now_ms: 99_000, color: false, reveal_clock: false };
            let lines = render(&v, w);
            assert!(lines.iter().all(|l| vlen(l) == w), "width {w}");
            assert!(lines.iter().any(|l| l.contains("RESULT")));
            assert!(lines.iter().any(|l| l.contains("✓")), "rows must match in the recording");
        }
    }
}
