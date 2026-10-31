### threads 2
| `SELECT id3, SUM(v1) FROM x GROUP BY id3 HAVING SUM(v1) < 0` | 1.028 | 1.037 | 1.045 | 1.972, 1.033, 1.044, 1.028, 1.035, 1.040, 1.045, 1.037, 1.038 |
### threads 4
| `SELECT id3, SUM(v1) FROM x GROUP BY id3 HAVING SUM(v1) < 0` | 0.780 | 0.791 | 0.797 | 0.794, 0.785, 0.797, 0.791, 0.796, 0.780, 0.791, 0.794, 0.786 |
### threads 6
| `SELECT id3, SUM(v1) FROM x GROUP BY id3 HAVING SUM(v1) < 0` | 1.319 | 1.458 | 1.494 | 1.302, 1.471, 1.398, 1.424, 1.463, 1.454, 1.471, 1.494, 1.319 |
### threads 8
| `SELECT id3, SUM(v1) FROM x GROUP BY id3 HAVING SUM(v1) < 0` | 1.473 | 1.566 | 1.614 | 1.522, 1.561, 1.564, 1.589, 1.527, 1.614, 1.573, 1.473, 1.567 |
### threads 11
| `SELECT id3, SUM(v1) FROM x GROUP BY id3 HAVING SUM(v1) < 0` | 1.164 | 1.540 | 1.652 | 1.249, 1.636, 1.464, 1.642, 1.203, 1.196, 1.652, 1.615, 1.164 |
