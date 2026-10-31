| query | min s | median s | max s | all runs (s, first = warm-up) |
|---|---|---|---|---|
| `SELECT id3, SUM(v1) FROM x GROUP BY id3 HAVING SUM(v1) < 0` | 1.004 | 1.005 | 1.005 | 0.996, 1.005, 1.004, 1.005 |
| `SELECT id3, SUM(v1) FROM x GROUP BY id3` | 1.636 | 1.644 | 1.654 | 1.646, 1.654, 1.644, 1.636 |
| `SELECT id3, SUM(v1) FROM x GROUP BY id3 ORDER BY 1 LIMIT 5` | 1.662 | 1.662 | 1.683 | 1.651, 1.683, 1.662, 1.662 |
