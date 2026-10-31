#!/usr/bin/env python3
"""Generate db-benchmark-shaped join data (tables x, small, medium, big) + schema.toml.

Shapes follow h2oai/db-benchmark "join":  with N = --rows
  x      N rows          id1 (1..N/1e6)  id2 (1..N/1e3)  id3 (1..N)  id4 id5 id6 (str)  v1 (f64)
  small  N/1e6 rows      id1  id4 (str)  v2 (f64)
  medium N/1e3 rows      id1  id2  id4  id5 (str)  v2
  big    N rows          id1  id2  id3  id4  id5  id6 (str)  v2
Keys in small/medium/big are unique (a permutation), so x JOIN dim is a foreign-key join.
Group-by-friendly cardinalities: id1 ~10 groups, id2 ~10^4, id3 ~N at N=1e7.

Pure standard library; deterministic for a given --seed.
usage: python3 scripts/gen_data.py --rows 1e7 [--out data] [--seed 42]
"""
import argparse
import os
import random
import sys
import time


def write_table(path, header, n, make_chunk, chunk=200_000):
    t0 = time.time()
    with open(path, "w", buffering=1 << 20, newline="\n") as f:
        f.write(",".join(header) + "\n")
        done = 0
        while done < n:
            k = min(chunk, n - done)
            f.write(make_chunk(done, k))
            done += k
    print(f"  {os.path.basename(path)}: {n:,} rows, {os.path.getsize(path) / 1e6:.0f} MB, {time.time() - t0:.1f}s", flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rows", type=float, default=1e7)
    ap.add_argument("--out", default="data")
    ap.add_argument("--seed", type=int, default=42)
    a = ap.parse_args()
    n = int(a.rows)
    n1 = max(1, n // 1_000_000)  # |small|
    n2 = max(1, n // 1_000)      # |medium|
    n3 = n                       # |big|
    rng = random.Random(a.seed)
    os.makedirs(a.out, exist_ok=True)
    print(f"generating N={n:,} into {a.out}/", flush=True)

    def fk_chunk(k, hi):
        return rng.choices(range(1, hi + 1), k=k)

    # ---- x
    def make_x(_, k):
        id1 = fk_chunk(k, n1)
        id2 = fk_chunk(k, n2)
        id3 = fk_chunk(k, n3)
        v1 = [round(rng.random() * 100, 6) for _ in range(k)]
        return "".join(
            f"{a},{b},{c},id{a:03d},id{b:06d},id{c:010d},{v}\n" for a, b, c, v in zip(id1, id2, id3, v1)
        )

    write_table(f"{a.out}/x.csv", ["id1", "id2", "id3", "id4", "id5", "id6", "v1"], n, make_x)

    # ---- dimension tables: unique shuffled keys
    def dim_perm(size, hi):
        # `size` distinct keys drawn from 1..hi
        return rng.sample(range(1, hi + 1), size) if size < hi else rng.sample(range(1, hi + 1), hi)

    id1s = dim_perm(n1, n1)
    write_table(
        f"{a.out}/small.csv", ["id1", "id4", "v2"], n1,
        lambda s, k: "".join(f"{i},id{i:03d},{round(rng.random() * 100, 6)}\n" for i in id1s[s:s + k]),
    )

    id2s = dim_perm(n2, n2)
    id1_of = lambda: rng.randint(1, n1)
    write_table(
        f"{a.out}/medium.csv", ["id1", "id2", "id4", "id5", "v2"], n2,
        lambda s, k: "".join(
            f"{id1_of()},{i},id{id1_of():03d},id{i:06d},{round(rng.random() * 100, 6)}\n" for i in id2s[s:s + k]
        ),
    )

    # big: id3 is a permutation of 1..N (unique), so memory is one list of ints
    id3s = list(range(1, n3 + 1))
    rng.shuffle(id3s)
    write_table(
        f"{a.out}/big.csv", ["id1", "id2", "id3", "id4", "id5", "id6", "v2"], n3,
        lambda s, k: "".join(
            f"{x},{y},{i},id{x:03d},id{y:06d},id{i:010d},{round(rng.random() * 100, 6)}\n"
            for i, x, y in zip(id3s[s:s + k], fk_chunk(k, n1), fk_chunk(k, n2))
        ),
    )
    del id3s

    with open(f"{a.out}/schema.toml", "w") as f:
        f.write(
            '[x]\nid1 = "i64"\nid2 = "i64"\nid3 = "i64"\nid4 = "str"\nid5 = "str"\nid6 = "str"\nv1 = "f64"\n\n'
            '[small]\nid1 = "i64"\nid4 = "str"\nv2 = "f64"\n\n'
            '[medium]\nid1 = "i64"\nid2 = "i64"\nid4 = "str"\nid5 = "str"\nv2 = "f64"\n\n'
            '[big]\nid1 = "i64"\nid2 = "i64"\nid3 = "i64"\nid4 = "str"\nid5 = "str"\nid6 = "str"\nv2 = "f64"\n'
        )
    print("done", flush=True)


if __name__ == "__main__":
    sys.exit(main())
