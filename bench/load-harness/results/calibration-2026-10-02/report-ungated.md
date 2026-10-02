# Load harness report

Host: Apple M4 Max (14 logical CPUs, 36864 MB RAM), macOS 27.0

Values are mean ± sample stddev over usable runs (CV in brackets). A headline metric (CPU, MSPT, footprint) with CV above 10%, fewer than 5 runs, or no measurement is flagged as too noisy to compare. Contention gate: a run counts only if other processes used at most 200% of a core on average and were above that in at most 10% of its seconds; contended runs are KEPT in these numbers (--allow-contended), so no group can be comparable.

Verdict: **NOT comparable yet, see flagged groups**

| Metric | neoforge 10 bots | pumpkin 10 bots | vanilla 10 bots |
|:--|--:|--:|--:|
| Valid runs | 5 | 5 | 5 |
| MSPT mean (ms) | 6.37 ± 2.41 (37.8%) ⚠ | 5.08 ± 2.26 (44.6%) ⚠ | 6.15 ± 2.62 (42.7%) ⚠ |
| MSPT p50 (ms) | 5.97 ± 2.38 (39.9%) | 3.33 ± 1.16 (34.8%) | 5.44 ± 2.87 (52.8%) |
| MSPT p95 (ms) | 11.30 ± 4.98 (44.1%) | 15.25 ± 8.94 (58.7%) | 11.99 ± 4.42 (36.9%) |
| MSPT p99 max (ms) | 63.48 ± 45.40 (71.5%) | 92.77 ± 22.78 (24.6%) | 157.38 ± 111.59 (70.9%) |
| TPS (client-observed) | 20.00 ± 0.00 (0.0%) | 20.00 ± 0.00 (0.0%) | 20.00 ± 0.00 (0.0%) |
| CPU mean (% of 1 core) | 24.58 ± 4.74 (19.3%) ⚠ | 23.97 ± 2.14 (8.9%) | 24.87 ± 3.99 (16.1%) ⚠ |
| CPU p95 (% of 1 core) | 33.66 ± 5.00 (14.9%) | 31.88 ± 3.03 (9.5%) | 34.88 ± 3.39 (9.7%) |
| Footprint mean (MB) | 2519.79 ± 33.43 (1.3%) | 258.77 ± 22.54 (8.7%) | 2450.40 ± 7.17 (0.3%) |
| Footprint peak (MB) | 2522.91 ± 33.78 (1.3%) | 271.55 ± 26.35 (9.7%) | 2471.91 ± 14.25 (0.6%) |
| Java live heap after GC (MB) | 325.02 ± 108.09 (33.3%) | n/a | 484.10 ± 342.49 (70.7%) |
| RSS mean (MB) | 1576.30 ± 160.40 (10.2%) | 166.26 ± 64.08 (38.5%) | 1232.83 ± 190.47 (15.4%) |
| RSS peak (MB) | 1829.97 ± 216.93 (11.9%) | 228.98 ± 85.97 (37.5%) | 1745.44 ± 216.57 (12.4%) |
| Chat RTT p50 (ms) | 40.40 ± 0.55 (1.4%) | 50.20 ± 11.71 (23.3%) | 40.20 ± 0.45 (1.1%) |
| Chat RTT p99 (ms) | 64.40 ± 22.17 (34.4%) | 113.60 ± 35.58 (31.3%) | 55.20 ± 5.26 (9.5%) |
| Bot swarm CPU (% of 1 core) | 9.99 ± 1.00 (10.0%) | 10.61 ± 1.07 (10.1%) | 9.69 ± 1.17 (12.1%) |
| Other host CPU (% of 1 core) | 767.56 ± 152.41 (19.9%) | 763.42 ± 64.14 (8.4%) | 754.49 ± 94.75 (12.6%) |
| Host memory available, min (MB) | 16469.67 ± 3600.12 (21.9%) | 13066.76 ± 2980.98 (22.8%) | 12871.78 ± 2132.38 (16.6%) |
| Places / breaks confirmed | 96% / 54% | 94% / 84% | 92% / 67% |

- **neoforge 10 bots** (neoforge 26.3.0.40-beta): seed 20250101 vd 8 sd 8 warmup 60s measure 120s heap 2G pre-touched
  - too noisy: cpu_pct_mean, mspt_mean
  - kept contended run neoforge-10bots-1790937654445.json: other processes used 915% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - kept contended run neoforge-10bots-1790938648718.json: other processes used 951% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - kept contended run neoforge-10bots-1790939640746.json: other processes used 639% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - kept contended run neoforge-10bots-1790941474870.json: other processes used 681% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - kept contended run neoforge-10bots-1790943515691.json: other processes used 652% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
- **pumpkin 10 bots** (pumpkin 742beaf6f release): seed 20250101 vd 8 sd 8 warmup 60s measure 120s heap 2G pre-touched
  - too noisy: mspt_mean
  - kept contended run pumpkin-10bots-1790937990158.json: other processes used 769% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - kept contended run pumpkin-10bots-1790938982246.json: other processes used 661% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - kept contended run pumpkin-10bots-1790939973315.json: other processes used 761% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - kept contended run pumpkin-10bots-1790940817441.json: other processes used 835% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - kept contended run pumpkin-10bots-1790941807234.json: other processes used 790% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
- **vanilla 10 bots** (vanilla 26.3): seed 20250101 vd 8 sd 8 warmup 60s measure 120s heap 2G pre-touched
  - too noisy: cpu_pct_mean, mspt_mean
  - kept contended run vanilla-10bots-1790937322630.json: other processes used 792% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - kept contended run vanilla-10bots-1790938316748.json: other processes used 735% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - kept contended run vanilla-10bots-1790939307609.json: other processes used 636% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - kept contended run vanilla-10bots-1790940298575.json: other processes used 892% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - kept contended run vanilla-10bots-1790941142635.json: other processes used 718% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
