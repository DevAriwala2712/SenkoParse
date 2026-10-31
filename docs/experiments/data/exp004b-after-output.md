| query | min s | median s | max s | all runs (s, first = warm-up) |
|---|---|---|---|---|
| `SELECT id3, SUM(v1) FROM x GROUP BY id3 HAVING SUM(v1) < 0` | 1.005 | 1.008 | 1.013 | 1.401, 1.005, 1.008, 1.013, 1.008 |
| `SELECT id3, SUM(v1) FROM x GROUP BY id3` | 1.285 | 1.289 | 1.292 | 1.289, 1.291, 1.285, 1.292, 1.288 |
| `SELECT id1, id2, id3, v1 FROM x WHERE id2 < 1000` | 0.188 | 0.190 | 0.192 | 0.190, 0.190, 0.188, 0.192, 0.190 |
