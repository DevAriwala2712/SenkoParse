| query | min s | median s | max s | all runs (s, first = warm-up) |
|---|---|---|---|---|
| `SELECT COUNT(*) FROM x` | 0.074 | 0.074 | 0.075 | 0.074, 0.075, 0.074, 0.074 |
| `SELECT SUM(v1) FROM x` | 0.217 | 0.217 | 0.217 | 0.217, 0.217, 0.217, 0.217 |
| `SELECT SUM(id1),SUM(id2),SUM(id3),SUM(v1) FROM x` | 0.241 | 0.241 | 0.242 | 0.242, 0.242, 0.241, 0.241 |
| `SELECT id1, SUM(v1) FROM x GROUP BY id1` | 0.211 | 0.212 | 0.212 | 0.212, 0.212, 0.212, 0.211 |
| `SELECT id2, COUNT(*), SUM(v1) FROM x GROUP BY id2` | 0.233 | 0.233 | 0.234 | 0.233, 0.233, 0.233, 0.234 |
| `SELECT id3, SUM(v1) FROM x GROUP BY id3` | 2.529 | 2.538 | 2.559 | 2.541, 2.538, 2.529, 2.559 |
| `SELECT s.id4, SUM(x.v1) FROM x JOIN small s ON x.id1 = s.id1 GROUP BY s.id4` | 0.257 | 0.262 | 0.264 | 0.265, 0.264, 0.257, 0.262 |
| `SELECT COUNT(*), SUM(x.v1 + b.v2) FROM x JOIN big b ON x.id3 = b.id3` | 1.744 | 1.752 | 1.764 | 1.741, 1.744, 1.752, 1.764 |
