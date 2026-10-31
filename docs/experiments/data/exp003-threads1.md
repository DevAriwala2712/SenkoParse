| query | min s | median s | max s | all runs (s, first = warm-up) |
|---|---|---|---|---|
| `SELECT COUNT(*) FROM x` | 0.245 | 0.248 | 0.249 | 1.176, 0.248, 0.249, 0.245 |
| `SELECT SUM(v1) FROM x` | 0.730 | 0.730 | 0.731 | 0.729, 0.730, 0.730, 0.731 |
| `SELECT SUM(id1),SUM(id2),SUM(id3),SUM(v1) FROM x` | 0.826 | 0.828 | 0.835 | 0.871, 0.828, 0.826, 0.835 |
| `SELECT id1, SUM(v1) FROM x GROUP BY id1` | 0.712 | 0.715 | 0.722 | 0.713, 0.715, 0.712, 0.722 |
| `SELECT id2, COUNT(*), SUM(v1) FROM x GROUP BY id2` | 0.779 | 0.780 | 0.816 | 0.776, 0.780, 0.779, 0.816 |
| `SELECT id3, SUM(v1) FROM x GROUP BY id3` | 4.497 | 4.526 | 4.536 | 4.634, 4.526, 4.497, 4.536 |
| `SELECT s.id4, SUM(x.v1) FROM x JOIN small s ON x.id1 = s.id1 GROUP BY s.id4` | 0.879 | 0.916 | 0.917 | 0.919, 0.916, 0.917, 0.879 |
| `SELECT COUNT(*), SUM(x.v1 + b.v2) FROM x JOIN big b ON x.id3 = b.id3` | 3.408 | 3.438 | 3.468 | 3.973, 3.408, 3.468, 3.438 |
