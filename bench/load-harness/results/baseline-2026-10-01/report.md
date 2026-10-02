# Load harness report

Host: Apple M4 Max (14 logical CPUs, 36864 MB RAM), macOS 27.0

Values are mean ± sample stddev over valid runs (CV in brackets). A headline metric (CPU, MSPT, RSS) with CV above 10% or fewer than 3 runs is flagged as too noisy to compare. Runs were not filtered on other load on the host; see the "Other host CPU" row.

Verdict: **NOT comparable yet, see flagged groups**

| Metric | neoforge 10 bots | neoforge 50 bots | pumpkin 10 bots | pumpkin 50 bots | vanilla 10 bots | vanilla 50 bots |
|:--|--:|--:|--:|--:|--:|--:|
| Valid runs | 3 | 3 | 3 | 3 | 3 | 3 |
| MSPT mean (ms) | 6.01 ± 0.93 (15.4%) ⚠ | 10.92 ± 0.79 (7.2%) | 5.94 ± 1.96 (33.0%) ⚠ | 9.74 ± 1.07 (11.0%) ⚠ | 7.08 ± 0.96 (13.5%) ⚠ | 10.89 ± 0.48 (4.4%) |
| MSPT p95 (ms) | 11.08 ± 2.26 (20.4%) | 16.17 ± 1.22 (7.6%) | 18.76 ± 7.95 (42.4%) | 23.89 ± 7.10 (29.7%) | 14.50 ± 2.99 (20.6%) | 16.96 ± 1.82 (10.7%) |
| MSPT p99 max (ms) | 70.17 ± 41.04 (58.5%) | 44.07 ± 11.35 (25.8%) | 87.60 ± 15.86 (18.1%) | 122.68 ± 34.24 (27.9%) | 135.33 ± 63.52 (46.9%) | 56.50 ± 24.70 (43.7%) |
| TPS (client-observed) | 20.00 ± 0.00 (0.0%) | 20.00 ± 0.00 (0.0%) | 20.00 ± 0.00 (0.0%) | 20.00 ± 0.00 (0.0%) | 20.00 ± 0.00 (0.0%) | 20.00 ± 0.00 (0.0%) |
| CPU mean (% of 1 core) | 23.09 ± 1.76 (7.6%) | 94.86 ± 5.54 (5.8%) | 21.62 ± 1.74 (8.0%) | 81.97 ± 1.55 (1.9%) | 23.70 ± 1.63 (6.9%) | 88.75 ± 8.17 (9.2%) |
| CPU p95 (% of 1 core) | 35.00 ± 0.27 (0.8%) | 115.36 ± 7.35 (6.4%) | 29.99 ± 2.65 (8.8%) | 104.67 ± 5.29 (5.1%) | 36.30 ± 0.29 (0.8%) | 108.54 ± 3.52 (3.2%) |
| RSS mean (MB) | 1161.90 ± 195.72 (16.8%) ⚠ | 1540.69 ± 21.78 (1.4%) | 134.49 ± 27.50 (20.5%) ⚠ | 143.28 ± 39.96 (27.9%) ⚠ | 850.01 ± 62.97 (7.4%) | 1473.82 ± 167.87 (11.4%) ⚠ |
| RSS peak (MB) | 1546.81 ± 92.46 (6.0%) | 1681.48 ± 13.28 (0.8%) | 211.10 ± 78.32 (37.1%) | 197.64 ± 57.62 (29.2%) | 1529.59 ± 199.64 (13.1%) | 1625.05 ± 21.99 (1.4%) |
| Chat RTT p50 (ms) | 38.67 ± 1.15 (3.0%) | 34.67 ± 9.24 (26.6%) | 63.00 ± 19.16 (30.4%) | 58.33 ± 10.97 (18.8%) | 39.00 ± 0.00 (0.0%) | 41.33 ± 0.58 (1.4%) |
| Chat RTT p99 (ms) | 61.00 ± 8.72 (14.3%) | 55.00 ± 8.54 (15.5%) | 144.67 ± 76.97 (53.2%) | 119.00 ± 28.69 (24.1%) | 159.00 ± 65.87 (41.4%) | 76.33 ± 46.50 (60.9%) |
| Bot swarm CPU (% of 1 core) | 9.84 ± 0.15 (1.5%) | 19.69 ± 0.81 (4.1%) | 11.01 ± 1.35 (12.2%) | 20.26 ± 0.60 (3.0%) | 9.77 ± 0.37 (3.8%) | 19.80 ± 0.58 (2.9%) |
| Other host CPU (% of 1 core) | 874.53 ± 76.36 (8.7%) | 716.87 ± 34.05 (4.7%) | 830.32 ± 144.08 (17.4%) | 808.00 ± 5.96 (0.7%) | 923.17 ± 142.24 (15.4%) | 789.36 ± 135.53 (17.2%) |
| Places / breaks confirmed | 95% / 63% | 92% / 62% | 89% / 83% | 88% / 79% | 94% / 56% | 92% / 63% |

- **neoforge 10 bots** (neoforge 26.3.0.40-beta): seed 20250101 vd 8 sd 8 warmup 60s measure 120s heap 2G
  - too noisy: mspt_mean, rss_mb_mean
- **neoforge 50 bots** (neoforge 26.3.0.40-beta): seed 20250101 vd 8 sd 8 warmup 60s measure 120s heap 2G
- **pumpkin 10 bots** (pumpkin 742beaf6f release): seed 20250101 vd 8 sd 8 warmup 60s measure 120s heap 2G
  - too noisy: mspt_mean, rss_mb_mean
- **pumpkin 50 bots** (pumpkin 742beaf6f release): seed 20250101 vd 8 sd 8 warmup 60s measure 120s heap 2G
  - too noisy: mspt_mean, rss_mb_mean
- **vanilla 10 bots** (vanilla 26.3): seed 20250101 vd 8 sd 8 warmup 60s measure 120s heap 2G
  - too noisy: mspt_mean
- **vanilla 50 bots** (vanilla 26.3): seed 20250101 vd 8 sd 8 warmup 60s measure 120s heap 2G
  - too noisy: rss_mb_mean
