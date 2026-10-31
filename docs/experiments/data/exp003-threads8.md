| query | min s | median s | max s | all runs (s, first = warm-up) |
|---|---|---|---|---|
| `SELECT COUNT(*) FROM x` | 0.055 | 0.056 | 0.057 | 0.054, 0.057, 0.056, 0.055 |
| `SELECT SUM(v1) FROM x` | 0.147 | 0.148 | 0.149 | 0.150, 0.149, 0.147, 0.148 |
| `SELECT SUM(id1),SUM(id2),SUM(id3),SUM(v1) FROM x` | 0.166 | 0.166 | 0.167 | 0.170, 0.166, 0.166, 0.167 |
| `SELECT id1, SUM(v1) FROM x GROUP BY id1` | 0.150 | 0.150 | 0.152 | 0.150, 0.150, 0.152, 0.150 |
| `SELECT id2, COUNT(*), SUM(v1) FROM x GROUP BY id2` | 0.165 | 0.165 | 0.165 | 0.168, 0.165, 0.165, 0.165 |
| `SELECT id3, SUM(v1) FROM x GROUP BY id3` | 2.330 | 2.344 | 2.344 | 2.340, 2.344, 2.344, 2.330 |
| `SELECT s.id4, SUM(x.v1) FROM x JOIN small s ON x.id1 = s.id1 GROUP BY s.id4` | 0.179 | 0.180 | 0.181 | 0.181, 0.179, 0.180, 0.181 |
| `SELECT COUNT(*), SUM(x.v1 + b.v2) FROM x JOIN big b ON x.id3 = b.id3` | 1.536 | 1.540 | 1.543 | 1.541, 1.536, 1.543, 1.540 |
