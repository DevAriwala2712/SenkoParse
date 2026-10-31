### BEFORE --threads 1
| query | min s | median s | max s | all runs (s, first = warm-up) |
|---|---|---|---|---|
| `SELECT id3, SUM(v1) FROM x GROUP BY id3` | 4.374 | 4.410 | 4.501 | 4.556, 4.422, 4.374, 4.501, 4.398 |
| `SELECT id2, COUNT(*), SUM(v1) FROM x GROUP BY id2` | 0.776 | 0.799 | 0.802 | 0.805, 0.799, 0.802, 0.776, 0.800 |
| `SELECT id1, SUM(v1) FROM x GROUP BY id1` | 0.711 | 0.724 | 0.752 | 0.709, 0.711, 0.733, 0.716, 0.752 |
| `SELECT id4, SUM(v1) FROM x GROUP BY id4` | 0.750 | 0.751 | 0.773 | 0.777, 0.773, 0.751, 0.750, 0.752 |
### BEFORE --threads 11
| query | min s | median s | max s | all runs (s, first = warm-up) |
|---|---|---|---|---|
| `SELECT id3, SUM(v1) FROM x GROUP BY id3` | 2.275 | 2.279 | 2.323 | 2.303, 2.323, 2.275, 2.276, 2.282 |
| `SELECT id2, COUNT(*), SUM(v1) FROM x GROUP BY id2` | 0.144 | 0.147 | 0.165 | 0.144, 0.144, 0.146, 0.147, 0.165 |
| `SELECT id1, SUM(v1) FROM x GROUP BY id1` | 0.128 | 0.129 | 0.130 | 0.127, 0.128, 0.130, 0.128, 0.129 |
| `SELECT id4, SUM(v1) FROM x GROUP BY id4` | 0.131 | 0.133 | 0.136 | 0.134, 0.136, 0.131, 0.132, 0.134 |
