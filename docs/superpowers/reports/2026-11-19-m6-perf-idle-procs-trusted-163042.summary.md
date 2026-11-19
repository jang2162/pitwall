## M6 measurement: idle-procs

- commit: `2b1754942eaeaee74d10f1a18030be83ec5efd46` (dirty false)
- os: macOS 27.2.1 (26B131)
- hardware: Mac17,9 · Apple M5 Pro · 15 CPU
- app: `target/release/perf-m6/Pitwall.app`
- definition: per run a fresh app on an empty trusted project (no run configuration, no debug); from first frame + 10s for 30s: `pitwall` (the app process — the Host is in it) idle and interrupt wakeups and CPU time (rusage deltas), its threads by name, and the app's descendant processes every 1 s; 3 runs

- verdict: **nothing spawned** (descendant processes 0, `run-ports` thread seen false)
- `pitwall` idle wakeups/s **0.17** (0.03 – 0.17, n=3), interrupt wakeups/s **7.49** (6.19 – 9.56, n=3), CPU ms/s **1.18** (0.58 – 1.44, n=3), threads **26.0** (26.0 – 30.0, n=3)
- app instance footprint **228.3** (228.3 – 228.3, n=1) MB, `pitwall` **69.5** (69.5 – 69.5, n=1) MB

| Run | cold ms | idle wk/s | intr wk/s | CPU ms/s | threads | descendants | excluded |
|---|---|---|---|---|---|---|---|
| 0 | 531.0 | 0.17 | 7.49 | 1.18 | 26 | 0 | window hidden |
| 1 | 511.0 | 0.03 | 6.19 | 0.58 | 26 | 0 | window hidden |
| 2 | 505.7 | 0.17 | 9.56 | 1.44 | 30 | 0 |  |

Threads of `pitwall` (run 0, end of window): `(unnamed)` × 2, `JavaScriptCore libpas scavenger` × 1, `Log work queue` × 1, `WebCore: Scrolling` × 1, `com.apple.NSEventThread` × 1, `main` × 1, `notify-rs fsevents loop` × 2, `pw-fs-watch` × 1, `pw-index-events` × 1, `tokio-rt-worker` × 15
