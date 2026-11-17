## M5 record-only measurement: cli-headless

- commit: `f42cf4ce7aa0ed86e89bfe7afbe45a286ddf4ce8` (dirty false)
- os: macOS 27.2.1 (26B131)
- hardware: Mac17,9 · Apple M5 Pro · 15 CPU
- app: `target/release/perf-m5/Pitwall.app`
- definition: `<bundle>/Contents/MacOS/pitwall tool project_list` wall clock from just before spawn to process exit, 1 warm-up + 20 calls back to back, no app (headless host in the CLI process)

- wall clock (spawn → exit): **9.3** (9.0 – 10.5, n=20) ms, warm-up 13.2 ms
- CLI log: `connected to the running app` × 0, `serving headless` × 21

| Call | ms | exit | stdout bytes | load 1-min |
|---|---|---|---|---|
| 0 (warm-up) | 13.2 | 0 | 344 | 4.0 |
| 1 | 9.9 | 0 | 344 | 4.0 |
| 2 | 9.5 | 0 | 344 | 4.0 |
| 3 | 9.5 | 0 | 344 | 4.0 |
| 4 | 9.6 | 0 | 344 | 4.0 |
| 5 | 9.8 | 0 | 344 | 4.0 |
| 6 | 9.1 | 0 | 344 | 4.0 |
| 7 | 9.1 | 0 | 344 | 4.0 |
| 8 | 9.3 | 0 | 344 | 4.0 |
| 9 | 9.6 | 0 | 344 | 4.0 |
| 10 | 9.1 | 0 | 344 | 4.0 |
| 11 | 9.0 | 0 | 344 | 4.0 |
| 12 | 9.0 | 0 | 344 | 4.0 |
| 13 | 9.4 | 0 | 344 | 4.0 |
| 14 | 9.1 | 0 | 344 | 4.0 |
| 15 | 9.2 | 0 | 344 | 4.0 |
| 16 | 9.3 | 0 | 344 | 4.0 |
| 17 | 10.5 | 0 | 344 | 4.0 |
| 18 | 9.7 | 0 | 344 | 4.0 |
| 19 | 9.1 | 0 | 344 | 4.0 |
| 20 | 9.1 | 0 | 344 | 4.0 |
