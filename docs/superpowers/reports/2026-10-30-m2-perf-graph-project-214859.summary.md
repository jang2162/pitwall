## Pitwall performance baseline

- build: release (`/Users/jang2162/IdeaProjects/pitwall/.worktrees/pitwall/m2-realcheck-run/target/release/bundle/macos/Pitwall.app`)
- commit: `bc3d80b51ac1f8ae5e108127e17ae82132314d29` (uncommitted changes)
- os: macOS 27.0.1 (26A434)
- hardware: Mac17,9 · Apple M5 Pro · 15 CPUs · 48 GiB
- runs: 5 measured (5 valid, 0 excluded) + 1 warm-up · idle = 10s after the first frame · MB = 10^6 bytes
- project: `/Users/jang2162/IdeaProjects/pitwall/.worktrees/pitwall/m2-realcheck-run/target/perf-git/graph`

| metric | median | min – max | target |
| --- | --- | --- | --- |
| cold start → project window first frame | 430 ms | 423 – 452 ms | < 1500 ms (within target) |
| idle memory (phys_footprint, app + WebKit) | 223.6 MB | 205.6 – 228.0 MB | < 300 MB (within target) |
| idle memory (RSS, same processes; reference) | 384.0 MB | 379.0 – 390.0 MB | |
| start → detached app process enters `main` | 89 ms | 87 – 97 ms | |

| run | cold start (ms) | app main (ms) | footprint (MB) | RSS (MB) | processes |
| --- | --- | --- | --- | --- | --- |
| 0 (warm-up) | 1300 | 945 | 229.0 | 385.4 | com.apple.WebKit.WebContent 132.1 · pitwall 58.2 · com.apple.WebKit.GPU 25.3 · com.apple.audio.SandboxHelper 5.7 · com.apple.WebKit.Networking 5.2 · SetStoreUpdateService 2.4 |
| 1 | 452 | 87 | 228.0 | 384.0 | com.apple.WebKit.WebContent 132.7 · pitwall 56.8 · com.apple.WebKit.GPU 25.1 · com.apple.audio.SandboxHelper 5.7 · com.apple.WebKit.Networking 5.2 · SetStoreUpdateService 2.4 |
| 2 | 433 | 97 | 223.6 | 382.9 | com.apple.WebKit.WebContent 132.7 · pitwall 56.1 · com.apple.WebKit.GPU 21.4 · com.apple.audio.SandboxHelper 5.7 · com.apple.WebKit.Networking 5.3 · SetStoreUpdateService 2.4 |
| 3 | 430 | 89 | 205.6 | 390.0 | com.apple.WebKit.WebContent 112.5 · pitwall 58.5 · com.apple.WebKit.GPU 21.2 · com.apple.audio.SandboxHelper 5.7 · com.apple.WebKit.Networking 5.2 · SetStoreUpdateService 2.4 |
| 4 | 423 | 87 | 225.2 | 385.0 | com.apple.WebKit.WebContent 131.7 · pitwall 58.9 · com.apple.WebKit.GPU 21.2 · com.apple.audio.SandboxHelper 5.7 · com.apple.WebKit.Networking 5.2 · SetStoreUpdateService 2.4 |
| 5 | 424 | 92 | 214.8 | 379.0 | com.apple.WebKit.WebContent 125.6 · pitwall 56.7 · com.apple.WebKit.GPU 19.1 · com.apple.audio.SandboxHelper 5.7 · com.apple.WebKit.Networking 5.3 · SetStoreUpdateService 2.4 |

Start: wall clock just before the harness spawns `<bundle>/Contents/MacOS/pitwall <project>` (CLI cold start of a registered project). End: the project window's `perf_first_frame` mark: second requestAnimationFrame after the first React commit, timestamped by the app on receipt. Memory: sum of phys_footprint (Activity Monitor's Memory column) over the processes whose responsible pid is the app instance (the app and the WebKit WebContent/GPU/Networking it requested); the launch is spawned with responsibility disclaimed so that this group is the instance alone; memory sampled idle_secs after the first-frame mark; a run whose window was hidden at any time before the sample, or whose screen was locked, is excluded. Cache: warm disk cache, new processes: no purge between runs; warm-up runs and excluded runs (window hidden before its first frame, screen locked) are recorded but left out of the medians.
