## Pitwall performance baseline

- build: release (`/Users/jang2162/IdeaProjects/pitwall/.worktrees/pitwall/m2-realcheck-run/target/release/bundle/macos/Pitwall.app`)
- commit: `2da2151d54c4eeb1ca8aad9533b30fb48fd804a9` (uncommitted changes)
- os: macOS 27.0.1 (26A434)
- hardware: Mac17,9 · Apple M5 Pro · 15 CPUs · 48 GiB
- runs: 5 measured (5 valid, 0 excluded) + 1 warm-up · idle = 10s after the first frame · MB = 10^6 bytes

| metric | median | min – max | target |
| --- | --- | --- | --- |
| cold start → project window first frame | 439 ms | 431 – 462 ms | < 1500 ms (within target) |
| idle memory (phys_footprint, app + WebKit) | 198.6 MB | 192.7 – 214.2 MB | < 300 MB (within target) |
| idle memory (RSS, same processes; reference) | 371.5 MB | 369.1 – 373.7 MB | |
| start → detached app process enters `main` | 85 ms | 81 – 94 ms | |

| run | cold start (ms) | app main (ms) | footprint (MB) | RSS (MB) | processes |
| --- | --- | --- | --- | --- | --- |
| 0 (warm-up) | 1322 | 816 | 227.1 | 391.2 | com.apple.WebKit.WebContent 141.4 · pitwall 47.5 · com.apple.WebKit.GPU 24.8 · com.apple.audio.SandboxHelper 5.7 · com.apple.WebKit.Networking 5.3 · SetStoreUpdateService 2.4 |
| 1 | 462 | 85 | 198.6 | 373.3 | com.apple.WebKit.WebContent 112.7 · pitwall 47.2 · com.apple.WebKit.GPU 25.3 · com.apple.audio.SandboxHelper 5.7 · com.apple.WebKit.Networking 5.2 · SetStoreUpdateService 2.4 |
| 2 | 456 | 94 | 213.7 | 371.5 | com.apple.WebKit.WebContent 132.7 · pitwall 46.6 · com.apple.WebKit.GPU 21.1 · com.apple.audio.SandboxHelper 5.7 · com.apple.WebKit.Networking 5.2 · SetStoreUpdateService 2.4 |
| 3 | 431 | 81 | 198.0 | 369.1 | com.apple.WebKit.WebContent 112.8 · pitwall 46.7 · com.apple.WebKit.GPU 25.2 · com.apple.audio.SandboxHelper 5.7 · com.apple.WebKit.Networking 5.3 · SetStoreUpdateService 2.4 |
| 4 | 437 | 81 | 192.7 | 373.7 | com.apple.WebKit.WebContent 112.2 · pitwall 45.9 · com.apple.WebKit.GPU 21.2 · com.apple.audio.SandboxHelper 5.7 · com.apple.WebKit.Networking 5.2 · SetStoreUpdateService 2.3 |
| 5 | 439 | 94 | 214.2 | 370.6 | com.apple.WebKit.WebContent 133.3 · pitwall 46.5 · com.apple.WebKit.GPU 21.0 · com.apple.audio.SandboxHelper 5.7 · com.apple.WebKit.Networking 5.3 · SetStoreUpdateService 2.4 |

Start: wall clock just before the harness spawns `<bundle>/Contents/MacOS/pitwall <project>` (CLI cold start of a registered project). End: the project window's `perf_first_frame` mark: second requestAnimationFrame after the first React commit, timestamped by the app on receipt. Memory: sum of phys_footprint (Activity Monitor's Memory column) over the processes whose responsible pid is the app instance (the app and the WebKit WebContent/GPU/Networking it requested); the launch is spawned with responsibility disclaimed so that this group is the instance alone; memory sampled idle_secs after the first-frame mark; a run whose window was hidden at any time before the sample, or whose screen was locked, is excluded. Cache: warm disk cache, new processes: no purge between runs; warm-up runs and excluded runs (window hidden before its first frame, screen locked) are recorded but left out of the medians.
