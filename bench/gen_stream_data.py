#!/usr/bin/env python3
"""EXP-005-stream data: a reconstruction of the shape in DuckDB issue #25165 (NOT the issue's own script).

8 columns (6 UUIDs, a quoted user agent with commas, an integer timestamp), a 50-row `ids` lookup table with
unique ids, ~5% of rows whose center_id is one of those ids. Writes into --out:
  schema.toml, ids.csv, a_1x.csv, a_2x.csv, a_5x.csv, a_10x.csv
2x/5x/10x repeat the body of 1x (header once). Deterministic (--seed).
usage: python3 bench/gen_stream_data.py --out DIR [--rows 1100000] [--seed 42]
"""
import argparse, os, random

COLS = ["id", "center_id", "user_id", "session_id", "device_id", "trace_id", "user_agent", "ts"]
UAS = [
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.1 Safari/605.1.15",
    "Mozilla/5.0 (X11; Linux x86_64; rv:121.0) Gecko/20100101 Firefox/121.0",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 17_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148",
    "Mozilla/5.0 (Linux; Android 13; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/119.0.0.0 Mobile Safari/537.36",
]

def uuids(rng, n):
    h = rng.randbytes(16 * n).hex()
    return [f"{h[i:i+8]}-{h[i+8:i+12]}-4{h[i+13:i+16]}-a{h[i+17:i+20]}-{h[i+20:i+32]}" for i in range(0, 32 * n, 32)]

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--rows", type=int, default=1_100_000)
    ap.add_argument("--seed", type=int, default=42)
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    rng = random.Random(a.seed)
    ids = uuids(rng, 50)
    with open(f"{a.out}/ids.csv", "w") as f:
        f.write("id\n" + "\n".join(ids) + "\n")
    with open(f"{a.out}/schema.toml", "w") as f:
        f.write("[a]\n" + "".join(f'{c} = "{"i64" if c == "ts" else "str"}"\n' for c in COLS) + '\n[ids]\nid = "str"\n')
    body = f"{a.out}/.body"
    with open(body, "w") as f:
        done = 0
        while done < a.rows:
            n = min(20000, a.rows - done)
            u = uuids(rng, 6 * n)
            out = []
            for i in range(n):
                r = u[6 * i: 6 * i + 6]
                if rng.random() < 0.05:
                    r[1] = ids[rng.randrange(50)]
                out.append(f'{r[0]},{r[1]},{r[2]},{r[3]},{r[4]},{r[5]},"{UAS[rng.randrange(5)]}",{1_600_000_000 + done + i}\n')
            f.write("".join(out))
            done += n
    header = ",".join(COLS) + "\n"
    for k in (1, 2, 5, 10):
        with open(f"{a.out}/a_{k}x.csv", "w") as f:
            f.write(header)
            for _ in range(k):
                with open(body) as b:
                    while chunk := b.read(1 << 24):
                        f.write(chunk)
    os.remove(body)
    for k in (1, 2, 5, 10):
        print(f"a_{k}x.csv {os.path.getsize(f'{a.out}/a_{k}x.csv') / 1e6:.1f} MB")

if __name__ == "__main__":
    main()
