#!/usr/bin/env python3
"""Regenerate docs/img/*.svg (no dependencies). Inputs: docs/experiments/data/exp005-stream-raw.jsonl for the stream
charts; the other numbers are copied from BENCHMARKS.md / EXP-003 (cited in docs/performance.md) and listed below.
Palette: first categorical slots of the validated reference palette (blue, orange, aqua; checked with --pairs all).
Charts sit on a fixed light card so they read the same in light and dark README themes."""
import json, os, statistics as st
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = f"{ROOT}/docs/img"
BLUE, ORANGE, AQUA, MAGENTA = "#2a78d6", "#eb6834", "#1baf7a", "#e87ba4"
INK, INK2, GRID, SURF = "#0b0b0b", "#52514e", "#e3e2dd", "#fcfcfb"
F = "font-family=\"system-ui,-apple-system,Segoe UI,Helvetica,Arial,sans-serif\""

def esc(s): return s.replace("&", "&amp;").replace("<", "&lt;")

def frame(w, h, title, desc, body, subtitle=""):
    sub = f'<text x="24" y="50" {F} font-size="12.5" fill="{INK2}">{esc(subtitle)}</text>' if subtitle else ""
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}" role="img" aria-labelledby="t d">'
            f'<title id="t">{esc(title)}</title><desc id="d">{esc(desc)}</desc>'
            f'<rect x="0.5" y="0.5" width="{w-1}" height="{h-1}" rx="10" fill="{SURF}" stroke="{GRID}"/>'
            f'<text x="24" y="32" {F} font-size="16" font-weight="600" fill="{INK}">{esc(title)}</text>{sub}{body}</svg>')

def nice(mx, n=4):
    step = mx / n
    mag = 10 ** (len(str(int(step))) - 1) if step >= 1 else 10 ** -1
    for m in (1, 2, 2.5, 5, 10):
        if step <= m * mag: step = m * mag; break
    return step

def line_chart(path, title, desc, subtitle, xs, series, xlabel, ylabel, xfmt, ymax=None, ideal=None, ylog=False, W=820, H=400):
    L, R, T, B = 74, 210, 76, 62
    pw, ph = W - L - R, H - T - B
    xmax = max(xs)
    ymaxv = ymax or max(max(s["y"]) for s in series)
    step = nice(ymaxv)
    ymaxv = step * (int(ymaxv / step) + (0 if ymaxv % step == 0 else 1))
    X = lambda v: L + pw * v / xmax
    Y = lambda v: T + ph - ph * v / ymaxv
    g = []
    v = 0
    while v <= ymaxv + 1e-9:
        g.append(f'<line x1="{L}" x2="{L+pw}" y1="{Y(v):.1f}" y2="{Y(v):.1f}" stroke="{GRID}"/>'
                 f'<text x="{L-8}" y="{Y(v)+4:.1f}" text-anchor="end" {F} font-size="11.5" fill="{INK2}">{v:g}</text>')
        v += step
    for x in xs:
        g.append(f'<line x1="{X(x):.1f}" x2="{X(x):.1f}" y1="{T+ph}" y2="{T+ph+5}" stroke="{INK2}"/>'
                 f'<text x="{X(x):.1f}" y="{T+ph+20}" text-anchor="middle" {F} font-size="11.5" fill="{INK2}">{xfmt(x)}</text>')
    g.append(f'<text x="{L+pw/2}" y="{H-14}" text-anchor="middle" {F} font-size="12" fill="{INK2}">{esc(xlabel)}</text>')
    g.append(f'<text transform="translate(18,{T+ph/2}) rotate(-90)" text-anchor="middle" {F} font-size="12" fill="{INK2}">{esc(ylabel)}</text>')
    if ideal:
        pts = " ".join(f"{X(x):.1f},{Y(ideal(x)):.1f}" for x in xs)
        g.append(f'<polyline points="{pts}" fill="none" stroke="{INK2}" stroke-width="1.5" stroke-dasharray="5 4"/>')
        g.append(f'<text x="{X(xs[-1])+8:.1f}" y="{Y(ideal(xs[-1]))+4:.1f}" {F} font-size="12" fill="{INK2}">ideal</text>')
    for s in series:
        pts = " ".join(f"{X(x):.1f},{Y(y):.1f}" for x, y in zip(xs, s["y"]))
        dash = ' stroke-dasharray="2 4"' if s.get("dotted") else ""
        g.append(f'<polyline points="{pts}" fill="none" stroke="{s["c"]}" stroke-width="2.5" stroke-linejoin="round" stroke-linecap="round"{dash}/>')
        for x, y in zip(xs, s["y"]):
            g.append(f'<circle cx="{X(x):.1f}" cy="{Y(y):.1f}" r="4.5" fill="{s["c"]}" stroke="{SURF}" stroke-width="2"><title>{esc(s["name"])}: {y:g} at {xfmt(x)}</title></circle>')
        g.append(f'<text x="{X(xs[-1])+10:.1f}" y="{Y(s["y"][-1])+(s.get("dy",0))+4:.1f}" {F} font-size="12.5" font-weight="600" fill="{INK}">{esc(s["name"])}</text>')
        g.append(f'<text x="{X(xs[-1])+10:.1f}" y="{Y(s["y"][-1])+(s.get("dy",0))+18:.1f}" {F} font-size="11.5" fill="{INK2}">{s["y"][-1]:g}</text>')
    open(path, "w").write(frame(W, H, title, desc, "".join(g), subtitle))

def hbars(path, title, desc, subtitle, cats, a, b, na, nb, unit, W=760):
    L, T, R = 190, 84, 70
    rowh = 38
    H = T + rowh * len(cats) + 46
    pw = W - L - R
    mx = max(a + b)
    step = nice(mx)
    mxv = step * (int(mx / step) + 1)
    g = []
    v = 0
    while v <= mxv + 1e-9:
        x = L + pw * v / mxv
        g.append(f'<line x1="{x:.1f}" x2="{x:.1f}" y1="{T-6}" y2="{T+rowh*len(cats)}" stroke="{GRID}"/>'
                 f'<text x="{x:.1f}" y="{T+rowh*len(cats)+16}" text-anchor="middle" {F} font-size="11.5" fill="{INK2}">{v:g}</text>')
        v += step
    g.append(f'<text x="{L+pw/2}" y="{H-8}" text-anchor="middle" {F} font-size="12" fill="{INK2}">{esc(unit)}</text>')
    g.append(f'<rect x="{L}" y="58" width="12" height="12" rx="2" fill="{ORANGE}"/><text x="{L+18}" y="68" {F} font-size="12" fill="{INK}">{esc(na)}</text>')
    g.append(f'<rect x="{L+210}" y="58" width="12" height="12" rx="2" fill="{BLUE}"/><text x="{L+228}" y="68" {F} font-size="12" fill="{INK}">{esc(nb)}</text>')
    for i, c in enumerate(cats):
        y = T + i * rowh
        g.append(f'<text x="{L-10}" y="{y+rowh/2+4:.1f}" text-anchor="end" {F} font-size="12.5" fill="{INK}">{esc(c)}</text>')
        for k, (val, col) in enumerate(((a[i], ORANGE), (b[i], BLUE))):
            w = pw * val / mxv
            by = y + 4 + k * 15
            g.append(f'<path d="M{L},{by} h{max(w-4,0):.1f} a4,4 0 0 1 4,4 v6 a4,4 0 0 1 -4,4 h-{max(w-4,0):.1f} z" fill="{col}"><title>{esc(c)}: {val:g} s</title></path>'
                     f'<text x="{L+w+6:.1f}" y="{by+11}" {F} font-size="11.5" fill="{INK2}">{val:g}</text>')
    open(path, "w").write(frame(W, H, title, desc, "".join(g), subtitle))

def main():
    os.makedirs(OUT, exist_ok=True)
    raw = [json.loads(l) for l in open(f"{ROOT}/docs/experiments/data/exp005-stream-raw.jsonl")]
    sizes = [("1x", 375), ("2x", 750), ("5x", 1874), ("10x", 3749)]
    def med(engine, key): return [round(st.median(r[key] for r in raw if r["engine"] == engine and r["size"] == s), 1) for s, _ in sizes]
    xs = [mb / 1000 for _, mb in sizes]
    gb = lambda x: f"{x:.1f} GB"
    line_chart(f"{OUT}/stream-rss.svg", "Peak memory vs piped input size",
               "Line chart. SenkoParse stays at 42 MiB from 0.4 to 3.7 GB; DuckDB piped is about 340 MiB; DuckDB by filename about 245 MiB.",
               "Peak RSS in MiB, median of 3 runs, query: 50-id join. Lower is better.",
               xs, [dict(name="DuckDB piped", c=ORANGE, y=med("duckdb pipe", "rss_mib"), dy=-6),
                    dict(name="DuckDB by filename", c=AQUA, y=med("duckdb file", "rss_mib"), dy=14),
                    dict(name="qe --stdin", c=BLUE, y=med("qe pipe", "rss_mib"))],
               "piped input size", "peak RSS (MiB)", gb, ymax=400)
    line_chart(f"{OUT}/stream-temp.svg", "Temp-file use vs piped input size",
               "Line chart. DuckDB piped writes 237 MiB to 5.6 GB of temp files as input grows; SenkoParse and DuckDB by filename write none.",
               "Peak size of the engine's temp directory in MiB (max of 3 runs). Lower is better.",
               xs, [dict(name="DuckDB piped", c=ORANGE, y=[round(max(r["temp_mib"] for r in raw if r["engine"] == "duckdb pipe" and r["size"] == s), 0) for s, _ in sizes]),
                    dict(name="qe --stdin, DuckDB by filename", c=BLUE, y=[0, 0, 0, 0], dy=-12)],
               "piped input size", "peak temp use (MiB)", gb, ymax=6000)
    line_chart(f"{OUT}/stream-wall.svg", "Wall time vs piped input size",
               "Line chart. SenkoParse takes 0.55 to 4.8 seconds; DuckDB by filename 0.9 to 7; DuckDB piped 2 to 70 seconds.",
               "Seconds, min of 3, 1 thread, Linux 4-core. At 3.7 GB: 4.78 s vs 69.8 s = 14.6x. One query.",
               xs, [dict(name="DuckDB 2.0.0 piped", c=ORANGE, y=[min(r["wall_s"] for r in raw if r["engine"] == "duckdb pipe" and r["size"] == s) for s, _ in sizes]),
                    dict(name="DuckDB 2.0.0 file", c=AQUA, y=[min(r["wall_s"] for r in raw if r["engine"] == "duckdb file" and r["size"] == s) for s, _ in sizes], dy=-6),
                    dict(name="qe --stdin", c=BLUE, y=[min(r["wall_s"] for r in raw if r["engine"] == "qe pipe" and r["size"] == s) for s, _ in sizes], dy=12)],
               "piped input size", "wall time (s)", gb, ymax=80)
    mac = [json.loads(l) for l in open(f"{ROOT}/docs/experiments/data/exp005-mac-m3pro-duckdb145.jsonl")]
    msz = [("1x", 375), ("5x", 1874)]
    mw = lambda e: [min(r["wall_s"] for r in mac if r["engine"] == e and r["size"] == s) for s, _ in msz]
    line_chart(f"{OUT}/stream-wall-mac.svg", "Wall time on an M3 Pro, DuckDB 1.4.5",
               "Line chart. On an Apple M3 Pro with DuckDB 1.4.5, SenkoParse takes 0.25 and 1.25 seconds at 0.4 and 1.9 GB; DuckDB piped 1.06 and 4.83; DuckDB by filename 0.49 and 2.13.",
               "Seconds, min of 2, 1 thread. qe vs DuckDB piped: 4.2x at 0.4 GB, 3.9x at 1.9 GB. One query.",
               [mb / 1000 for _, mb in msz], [dict(name="DuckDB 1.4.5 piped", c=ORANGE, y=mw("duckdb pipe")),
                    dict(name="DuckDB 1.4.5 file", c=AQUA, y=mw("duckdb file"), dy=-6),
                    dict(name="qe --stdin", c=BLUE, y=mw("qe pipe"), dy=12)],
               "piped input size", "wall time (s)", gb, ymax=6)
    cats = ["Q1 global aggregate", "Q2 group by id1 (10)", "Q3 group by id2 (1e4)", "Q4 group by id3 (6e6)", "Q5 group by string id4",
            "Q6 filter + aggregate", "Q7 join small + group", "Q8 join medium + group", "Q9 join big (1e7 x 1e7)"]
    m1 = [0.80, 0.74, 0.77, 2.54, 0.94, 0.74, 0.91, 0.98, 3.59]
    now = [0.14, 0.13, 0.13, 0.67, 0.23, 0.13, 0.16, 0.18, 1.51]
    hbars(f"{OUT}/perf-progress.svg", "File mode: M1 baseline vs today",
          "Horizontal grouped bars, seconds per query on 1e7 rows. Today is 2.4 to 5.9 times faster than M1 on every query.",
          "1e7 rows, 527 MB, Apple M3 Pro, 11 threads, warm cache. Minimum of 3 runs. Lower is better.",
          cats, m1, now, "M1: single thread", "Today: parallel + integer-key aggregation", "seconds")
    th = [1, 2, 4, 8, 11]
    sp = lambda t: [round(t[0] / x, 2) for x in t]
    line_chart(f"{OUT}/perf-scaling.svg", "Thread scaling of the chunked scan (EXP-003)",
               "Line chart of speedup versus threads. Scan-bound queries reach 5 to 6 times at 11 threads; the 6e6-group aggregate and the big join scale poorly.",
               "Speedup over 1 thread, median of 4 runs, Apple M3 Pro (5 performance + 6 efficiency cores). Higher is better.",
               th, [dict(name="SUM(v1), small joins", c=BLUE, y=sp([0.730, 0.390, 0.217, 0.148, 0.127])),
                    dict(name="join big", c=ORANGE, y=sp([3.438, 2.380, 1.752, 1.540, 1.472]), dy=-14),
                    dict(name="GROUP BY id3 (before EXP-004)", c=MAGENTA, y=sp([4.526, 3.158, 2.538, 2.344, 2.311]), dy=16)],
               "threads", "speedup (x)", lambda x: f"{x}", ymax=12, ideal=lambda x: x)

if __name__ == "__main__":
    main()
