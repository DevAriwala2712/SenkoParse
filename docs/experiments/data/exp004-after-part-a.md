### AFTER --threads 1
| query | min s | median s | max s | all runs (s, first = warm-up) |
|---|---|---|---|---|
| `SELECT id3, SUM(v1) FROM x GROUP BY id3` | 3.085 | 3.100 | 3.119 | 3.576, 3.119, 3.085, 3.101, 3.100 |
| `SELECT id2, COUNT(*), SUM(v1) FROM x GROUP BY id2` | 0.702 | 0.740 | 0.741 | 0.741, 0.740, 0.702, 0.741, 0.739 |
| `SELECT id1, SUM(v1) FROM x GROUP BY id1` | 0.666 | 0.668 | 0.701 | 0.667, 0.668, 0.701, 0.666, 0.668 |
| `SELECT id4, SUM(v1) FROM x GROUP BY id4` | 0.761 | 0.781 | 0.801 | 0.761, 0.801, 0.761, 0.801, 0.761 |
### AFTER --threads 11
| query | min s | median s | max s | all runs (s, first = warm-up) |
|---|---|---|---|---|
| `SELECT id3, SUM(v1) FROM x GROUP BY id3` | 1.646 | 1.651 | 1.658 | 1.656, 1.652, 1.646, 1.658, 1.651 |
| `SELECT id2, COUNT(*), SUM(v1) FROM x GROUP BY id2` | 0.123 | 0.130 | 0.143 | 0.128, 0.143, 0.123, 0.129, 0.131 |
| `SELECT id1, SUM(v1) FROM x GROUP BY id1` | 0.120 | 0.122 | 0.122 | 0.120, 0.122, 0.122, 0.120, 0.122 |
| `SELECT id4, SUM(v1) FROM x GROUP BY id4` | 0.130 | 0.134 | 0.142 | 0.142, 0.136, 0.131, 0.130, 0.142 |
