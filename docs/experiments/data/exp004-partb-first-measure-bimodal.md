### PART B --threads 1
| query | min s | median s | max s | all runs (s, first = warm-up) |
|---|---|---|---|---|
| `SELECT id3, SUM(v1) FROM x GROUP BY id3 HAVING SUM(v1) < 0` | 1.680 | 1.728 | 1.731 | 2.078, 1.729, 1.731, 1.727, 1.680 |
| `SELECT id3, SUM(v1) FROM x GROUP BY id3` | 1.974 | 1.979 | 2.007 | 2.019, 2.007, 1.981, 1.974, 1.977 |
| `SELECT id2, COUNT(*), SUM(v1) FROM x GROUP BY id2` | 0.720 | 0.735 | 0.749 | 0.723, 0.748, 0.749, 0.720, 0.721 |
| `SELECT id1, SUM(v1) FROM x GROUP BY id1` | 0.679 | 0.686 | 0.687 | 0.716, 0.679, 0.686, 0.687, 0.686 |
### PART B --threads 4
| query | min s | median s | max s | all runs (s, first = warm-up) |
|---|---|---|---|---|
| `SELECT id3, SUM(v1) FROM x GROUP BY id3 HAVING SUM(v1) < 0` | 0.769 | 0.777 | 0.784 | 0.773, 0.769, 0.784, 0.775, 0.779 |
| `SELECT id3, SUM(v1) FROM x GROUP BY id3` | 1.067 | 1.069 | 1.071 | 1.059, 1.068, 1.069, 1.067, 1.071 |
| `SELECT id2, COUNT(*), SUM(v1) FROM x GROUP BY id2` | 0.222 | 0.223 | 0.224 | 0.224, 0.222, 0.224, 0.224, 0.222 |
| `SELECT id1, SUM(v1) FROM x GROUP BY id1` | 0.199 | 0.199 | 0.201 | 0.202, 0.199, 0.201, 0.199, 0.200 |
### PART B --threads 11
| query | min s | median s | max s | all runs (s, first = warm-up) |
|---|---|---|---|---|
| `SELECT id3, SUM(v1) FROM x GROUP BY id3 HAVING SUM(v1) < 0` | 1.144 | 1.637 | 1.666 | 1.485, 1.144, 1.666, 1.623, 1.650 |
| `SELECT id3, SUM(v1) FROM x GROUP BY id3` | 1.487 | 1.930 | 1.976 | 1.962, 1.487, 1.935, 1.926, 1.976 |
| `SELECT id2, COUNT(*), SUM(v1) FROM x GROUP BY id2` | 0.160 | 0.167 | 0.188 | 0.171, 0.172, 0.188, 0.162, 0.160 |
| `SELECT id1, SUM(v1) FROM x GROUP BY id1` | 0.120 | 0.123 | 0.124 | 0.119, 0.120, 0.124, 0.124, 0.122 |
