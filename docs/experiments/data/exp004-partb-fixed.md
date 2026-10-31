### PART B (fixed) --threads 1
| query | min s | median s | max s | all runs (s, first = warm-up) |
|---|---|---|---|---|
| `SELECT id3, SUM(v1) FROM x GROUP BY id3 HAVING SUM(v1) < 0` | 1.787 | 1.827 | 1.851 | 2.328, 1.787, 1.830, 1.827, 1.822, 1.851 |
| `SELECT id3, SUM(v1) FROM x GROUP BY id3` | 2.116 | 2.151 | 2.265 | 2.233, 2.265, 2.172, 2.151, 2.132, 2.116 |
| `SELECT id2, COUNT(*), SUM(v1) FROM x GROUP BY id2` | 0.754 | 0.763 | 0.767 | 0.773, 0.763, 0.767, 0.763, 0.754, 0.754 |
| `SELECT id1, SUM(v1) FROM x GROUP BY id1` | 0.718 | 0.718 | 0.719 | 0.719, 0.718, 0.718, 0.719, 0.719, 0.718 |
### PART B (fixed) --threads 2
| query | min s | median s | max s | all runs (s, first = warm-up) |
|---|---|---|---|---|
| `SELECT id3, SUM(v1) FROM x GROUP BY id3 HAVING SUM(v1) < 0` | 0.938 | 0.949 | 0.956 | 0.936, 0.950, 0.949, 0.946, 0.938, 0.956 |
| `SELECT id3, SUM(v1) FROM x GROUP BY id3` | 1.219 | 1.236 | 1.246 | 1.220, 1.219, 1.246, 1.239, 1.236, 1.235 |
| `SELECT id2, COUNT(*), SUM(v1) FROM x GROUP BY id2` | 0.402 | 0.403 | 0.403 | 0.403, 0.403, 0.403, 0.402, 0.402, 0.403 |
| `SELECT id1, SUM(v1) FROM x GROUP BY id1` | 0.386 | 0.386 | 0.386 | 0.386, 0.386, 0.386, 0.386, 0.386, 0.386 |
### PART B (fixed) --threads 4
| query | min s | median s | max s | all runs (s, first = warm-up) |
|---|---|---|---|---|
| `SELECT id3, SUM(v1) FROM x GROUP BY id3 HAVING SUM(v1) < 0` | 0.516 | 0.524 | 0.572 | 0.528, 0.524, 0.526, 0.522, 0.516, 0.572 |
| `SELECT id3, SUM(v1) FROM x GROUP BY id3` | 0.810 | 0.820 | 0.826 | 0.819, 0.821, 0.812, 0.810, 0.820, 0.826 |
| `SELECT id2, COUNT(*), SUM(v1) FROM x GROUP BY id2` | 0.211 | 0.212 | 0.213 | 0.213, 0.212, 0.212, 0.213, 0.211, 0.212 |
| `SELECT id1, SUM(v1) FROM x GROUP BY id1` | 0.202 | 0.211 | 0.228 | 0.202, 0.202, 0.202, 0.228, 0.211, 0.213 |
### PART B (fixed) --threads 8
| query | min s | median s | max s | all runs (s, first = warm-up) |
|---|---|---|---|---|
| `SELECT id3, SUM(v1) FROM x GROUP BY id3 HAVING SUM(v1) < 0` | 0.420 | 0.485 | 0.563 | 0.523, 0.485, 0.563, 0.460, 0.420, 0.530 |
| `SELECT id3, SUM(v1) FROM x GROUP BY id3` | 0.771 | 0.990 | 2.587 | 0.765, 0.771, 0.780, 0.990, 2.587, 1.271 |
| `SELECT id2, COUNT(*), SUM(v1) FROM x GROUP BY id2` | 0.162 | 0.180 | 0.217 | 0.299, 0.217, 0.216, 0.180, 0.162, 0.166 |
| `SELECT id1, SUM(v1) FROM x GROUP BY id1` | 0.152 | 0.216 | 0.232 | 0.149, 0.152, 0.160, 0.217, 0.232, 0.216 |
### PART B (fixed) --threads 11
| query | min s | median s | max s | all runs (s, first = warm-up) |
|---|---|---|---|---|
| `SELECT id3, SUM(v1) FROM x GROUP BY id3 HAVING SUM(v1) < 0` | 0.397 | 0.421 | 0.515 | 0.556, 0.515, 0.421, 0.397, 0.409, 0.456 |
| `SELECT id3, SUM(v1) FROM x GROUP BY id3` | 0.738 | 0.799 | 1.209 | 0.731, 0.748, 0.738, 0.799, 1.157, 1.209 |
| `SELECT id2, COUNT(*), SUM(v1) FROM x GROUP BY id2` | 0.148 | 0.198 | 0.220 | 0.162, 0.198, 0.220, 0.176, 0.148, 0.203 |
| `SELECT id1, SUM(v1) FROM x GROUP BY id1` | 0.132 | 0.141 | 0.155 | 0.139, 0.148, 0.155, 0.141, 0.132, 0.136 |
