| query | min s | median s | max s | all runs (s, first = warm-up) |
|---|---|---|---|---|
| `SELECT COUNT(*) FROM x` | 0.133 | 0.135 | 0.136 | 0.133, 0.133, 0.136, 0.135 |
| `SELECT SUM(v1) FROM x` | 0.388 | 0.390 | 0.398 | 0.397, 0.388, 0.390, 0.398 |
| `SELECT SUM(id1),SUM(id2),SUM(id3),SUM(v1) FROM x` | 0.459 | 0.462 | 0.463 | 0.463, 0.459, 0.462, 0.463 |
| `SELECT id1, SUM(v1) FROM x GROUP BY id1` | 0.393 | 0.397 | 0.400 | 0.403, 0.400, 0.393, 0.397 |
| `SELECT id2, COUNT(*), SUM(v1) FROM x GROUP BY id2` | 0.431 | 0.439 | 0.440 | 0.441, 0.440, 0.439, 0.431 |
| `SELECT id3, SUM(v1) FROM x GROUP BY id3` | 3.137 | 3.158 | 3.170 | 3.220, 3.170, 3.137, 3.158 |
| `SELECT s.id4, SUM(x.v1) FROM x JOIN small s ON x.id1 = s.id1 GROUP BY s.id4` | 0.493 | 0.498 | 0.510 | 0.501, 0.493, 0.510, 0.498 |
| `SELECT COUNT(*), SUM(x.v1 + b.v2) FROM x JOIN big b ON x.id3 = b.id3` | 2.346 | 2.380 | 2.381 | 2.376, 2.380, 2.346, 2.381 |
