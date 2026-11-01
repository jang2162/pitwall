## Pitwall performance baseline

- build: release (`/Users/jang2162/IdeaProjects/pitwall/.worktrees/pitwall/m3-21-perf/target/release/perf-m3/default/Pitwall.app`)
- commit: `dfe9ce9be42760f560171e7eda1038cff873484d`
- os: macOS 27.0.1 (26A434)
- hardware: Mac17,9 · Apple M5 Pro · 15 CPUs · 48 GiB
- runs: 5 measured (5 valid, 0 excluded) + 1 warm-up · idle = 10s after the first frame · MB = 10^6 bytes
- label: default bundle (release, no perf-fake-db)

| metric | median | min – max | target |
| --- | --- | --- | --- |
| cold start → project window first frame | 427 ms | 418 – 429 ms | < 1500 ms (within target) |
| idle memory (phys_footprint, app + WebKit) | 232.6 MB | 211.4 – 238.9 MB | < 300 MB (within target) |
| idle memory (RSS, same processes; reference) | 405.4 MB | 398.7 – 407.9 MB | |
| start → detached app process enters `main` | 79 ms | 76 – 81 ms | |

| run | cold start (ms) | app main (ms) | footprint (MB) | RSS (MB) | processes |
| --- | --- | --- | --- | --- | --- |
| 0 (warm-up) | 1453 | 1096 | 235.5 | 407.7 | com.apple.WebKit.WebContent 151.9 · pitwall 49.1 · com.apple.WebKit.GPU 21.2 · com.apple.audio.SandboxHelper 5.7 · com.apple.WebKit.Networking 5.1 · SetStoreUpdateService 2.5 |
| 1 | 428 | 79 | 211.4 | 399.7 | com.apple.WebKit.WebContent 122.3 · pitwall 50.5 · com.apple.WebKit.GPU 25.2 · com.apple.audio.SandboxHelper 5.7 · com.apple.WebKit.Networking 5.2 · SetStoreUpdateService 2.5 |
| 2 | 418 | 81 | 228.3 | 398.7 | com.apple.WebKit.WebContent 140.1 · pitwall 49.5 · com.apple.WebKit.GPU 25.3 · com.apple.audio.SandboxHelper 5.7 · com.apple.WebKit.Networking 5.2 · SetStoreUpdateService 2.5 |
| 3 | 429 | 80 | 237.3 | 407.9 | com.apple.WebKit.WebContent 150.3 · pitwall 48.3 · com.apple.WebKit.GPU 25.2 · com.apple.audio.SandboxHelper 5.7 · com.apple.WebKit.Networking 5.1 · SetStoreUpdateService 2.6 |
| 4 | 421 | 76 | 232.6 | 405.4 | com.apple.WebKit.WebContent 149.7 · pitwall 48.2 · com.apple.WebKit.GPU 21.3 · com.apple.audio.SandboxHelper 5.7 · com.apple.WebKit.Networking 5.1 · SetStoreUpdateService 2.5 |
| 5 | 427 | 78 | 238.9 | 405.9 | com.apple.WebKit.WebContent 151.5 · pitwall 48.6 · com.apple.WebKit.GPU 25.2 · com.apple.audio.SandboxHelper 5.7 · com.apple.WebKit.Networking 5.2 · SetStoreUpdateService 2.5 |

Start: wall clock just before the harness spawns `<bundle>/Contents/MacOS/pitwall <project>` (CLI cold start of a registered project). End: the project window's `perf_first_frame` mark: second requestAnimationFrame after the first React commit, timestamped by the app on receipt. Memory: sum of phys_footprint (Activity Monitor's Memory column) over the processes whose responsible pid is the app instance (the app and the WebKit WebContent/GPU/Networking it requested); the launch is spawned with responsibility disclaimed so that this group is the instance alone; memory sampled idle_secs after the first-frame mark; a run whose window was hidden at any time before the sample, or whose screen was locked, is excluded. Cache: warm disk cache, new processes: no purge between runs; warm-up runs and excluded runs (window hidden before its first frame, screen locked) are recorded but left out of the medians.
