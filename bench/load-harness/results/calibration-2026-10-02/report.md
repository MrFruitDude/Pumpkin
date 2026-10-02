# Load harness report

Host: Apple M4 Max (14 logical CPUs, 36864 MB RAM), macOS 27.0

Values are mean ± sample stddev over usable runs (CV in brackets). A headline metric (CPU, MSPT, footprint) with CV above 10%, fewer than 5 runs, or no measurement is flagged as too noisy to compare. Contention gate: a run counts only if other processes used at most 200% of a core on average and were above that in at most 10% of its seconds; contended runs are left out and listed below.

Verdict: **NOT comparable yet, see flagged groups**

| Metric | neoforge 10 bots | pumpkin 10 bots | vanilla 10 bots |
|:--|--:|--:|--:|
| Valid runs | 0 | 0 | 0 |
| MSPT mean (ms) | n/a | n/a | n/a |
| MSPT p50 (ms) | n/a | n/a | n/a |
| MSPT p95 (ms) | n/a | n/a | n/a |
| MSPT p99 max (ms) | n/a | n/a | n/a |
| TPS (client-observed) | n/a | n/a | n/a |
| CPU mean (% of 1 core) | n/a | n/a | n/a |
| CPU p95 (% of 1 core) | n/a | n/a | n/a |
| Footprint mean (MB) | n/a | n/a | n/a |
| Footprint peak (MB) | n/a | n/a | n/a |
| Java live heap after GC (MB) | n/a | n/a | n/a |
| RSS mean (MB) | n/a | n/a | n/a |
| RSS peak (MB) | n/a | n/a | n/a |
| Chat RTT p50 (ms) | n/a | n/a | n/a |
| Chat RTT p99 (ms) | n/a | n/a | n/a |
| Bot swarm CPU (% of 1 core) | n/a | n/a | n/a |
| Other host CPU (% of 1 core) | n/a | n/a | n/a |
| Host memory available, min (MB) | n/a | n/a | n/a |
| Places / breaks confirmed | 0% / 0% | 0% / 0% | 0% / 0% |

- **neoforge 10 bots** (neoforge 26.3.0.40-beta): 
  - too noisy: cpu_pct_mean, mspt_mean, footprint_mb_mean
  - excluded contended run neoforge-10bots-1790937654445.json: other processes used 915% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - excluded contended run neoforge-10bots-1790938648718.json: other processes used 951% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - excluded contended run neoforge-10bots-1790939640746.json: other processes used 639% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - excluded contended run neoforge-10bots-1790941474870.json: other processes used 681% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - excluded contended run neoforge-10bots-1790943515691.json: other processes used 652% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
- **pumpkin 10 bots** (pumpkin 742beaf6f release): 
  - too noisy: cpu_pct_mean, mspt_mean, footprint_mb_mean
  - excluded contended run pumpkin-10bots-1790937990158.json: other processes used 769% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - excluded contended run pumpkin-10bots-1790938982246.json: other processes used 661% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - excluded contended run pumpkin-10bots-1790939973315.json: other processes used 761% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - excluded contended run pumpkin-10bots-1790940817441.json: other processes used 835% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - excluded contended run pumpkin-10bots-1790941807234.json: other processes used 790% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
- **vanilla 10 bots** (vanilla 26.3): 
  - too noisy: cpu_pct_mean, mspt_mean, footprint_mb_mean
  - excluded contended run vanilla-10bots-1790937322630.json: other processes used 792% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - excluded contended run vanilla-10bots-1790938316748.json: other processes used 735% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - excluded contended run vanilla-10bots-1790939307609.json: other processes used 636% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - excluded contended run vanilla-10bots-1790940298575.json: other processes used 892% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
  - excluded contended run vanilla-10bots-1790941142635.json: other processes used 718% of a core on average (limit 200%); other load above 200% in 100% of seconds (limit 10%)
