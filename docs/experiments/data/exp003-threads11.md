| query | min s | median s | max s | all runs (s, first = warm-up) |
|---|---|---|---|---|
| `SELECT COUNT(*) FROM x` | 0.048 | 0.049 | 0.049 | 0.048, 0.049, 0.049, 0.048 |
| `SELECT SUM(v1) FROM x` | 0.125 | 0.127 | 0.131 | 0.126, 0.131, 0.125, 0.127 |
| `SELECT SUM(id1),SUM(id2),SUM(id3),SUM(v1) FROM x` | 0.145 | 0.147 | 0.149 | 0.144, 0.147, 0.145, 0.149 |
| `SELECT id1, SUM(v1) FROM x GROUP BY id1` | 0.128 | 0.129 | 0.131 | 0.131, 0.131, 0.129, 0.128 |
| `SELECT id2, COUNT(*), SUM(v1) FROM x GROUP BY id2` | 0.142 | 0.146 | 0.148 | 0.149, 0.142, 0.146, 0.148 |
| `SELECT id3, SUM(v1) FROM x GROUP BY id3` | 2.302 | 2.311 | 2.364 | 2.316, 2.364, 2.311, 2.302 |
| `SELECT s.id4, SUM(x.v1) FROM x JOIN small s ON x.id1 = s.id1 GROUP BY s.id4` | 0.153 | 0.155 | 0.159 | 0.150, 0.159, 0.153, 0.155 |
| `SELECT COUNT(*), SUM(x.v1 + b.v2) FROM x JOIN big b ON x.id3 = b.id3` | 1.461 | 1.472 | 1.477 | 1.469, 1.472, 1.461, 1.477 |
