# 성능 기준선 — M0-20

이 문서는 잰 시점의 기록이다. 지금의 측정 방법은 `CONTRIBUTING.md` 「성능 기준선」 절, 목표는 `docs/design/06-roadmap.md` 「성능 목표 (검증 기준)」 절이 정본이다. 기계가 읽는 원본은 같은 이름의 `2026-10-18-perf-baseline-report.json`(`pw-perf` 의 `result.json` 그대로)이다.

## 조건

| 항목 | 값 |
| --- | --- |
| 명령 | `mise run perf -- --out target/perf/baseline` |
| 빌드 | release 번들 `target/release/bundle/macos/Pitwall.app` (태스크가 그 자리에서 다시 만든 것) |
| 커밋 | `c1263c4840951a9d98fe10de53beb7b85ee2431b`, 커밋하지 않은 변경 없음 |
| 기기 | Mac17,9 · Apple M5 Pro · 15 CPU · 48 GiB, macOS 27.0.1 (26A434) |
| 반복 | 준비 1회 + 측정 5회, 유휴 = 첫 프레임 10초 뒤 |
| 캐시 | warm 디스크 캐시의 새 프로세스(회 사이에 `purge` 없음) |
| 같은 시각의 환경 | 사람이 기기를 쓰는 중이었고, 다른 워크트리(M0-16)의 debug 앱이 떠 있었다 — 그 창이 측정 창을 가린 회가 하나 나와 제외됐다 |

## 결과

| 지표 | 중앙값 | 범위 | 목표 |
| --- | --- | --- | --- |
| 콜드 스타트 → 프로젝트 창 첫 프레임 | 468 ms | 421 – 472 ms (4회) | < 1500 ms — 목표 안 |
| 유휴 메모리(`phys_footprint`, 앱 + WebKit) | 119.8 MiB | 118.5 – 120.0 MiB (4회) | < 300 MiB — 목표 안 |
| 유휴 메모리(RSS, 같은 프로세스, 참고) | 259.3 MiB | 257.3 – 262.7 MiB | |
| 시작 → 떼어 띄운 앱이 `main` 에 들어온 때 | 82 ms | 79 – 87 ms | |

| 회 | 콜드 스타트 (ms) | 앱 main (ms) | footprint (MiB) | RSS (MiB) | 프로세스별 footprint (MiB) |
| --- | --- | --- | --- | --- | --- |
| 0 (준비) | 780 | 315 | 119.1 | 268.2 | WebContent 54.2 · pitwall 35.9 · GPU 16.2 · audio.SandboxHelper 5.6 · Networking 4.8 · SetStoreUpdateService 2.3 |
| 1 | 469 | 82 | 120.0 | 258.0 | WebContent 54.3 · pitwall 35.8 · GPU 17.0 · audio.SandboxHelper 5.5 · Networking 4.9 · SetStoreUpdateService 2.4 |
| 2 (제외: 첫 프레임 전에 창이 가려짐) | 3250 | 95 | 112.3 | 257.4 | WebContent 46.1 · pitwall 36.7 · GPU 16.7 · audio.SandboxHelper 5.5 · Networking 4.8 · SetStoreUpdateService 2.5 |
| 3 | 421 | 79 | 118.5 | 260.5 | WebContent 51.3 · pitwall 37.3 · GPU 17.1 · audio.SandboxHelper 5.5 · Networking 4.9 · SetStoreUpdateService 2.4 |
| 4 | 472 | 87 | 119.7 | 262.7 | WebContent 51.6 · pitwall 37.4 · GPU 18.0 · audio.SandboxHelper 5.5 · Networking 4.8 · SetStoreUpdateService 2.4 |
| 5 | 467 | 82 | 119.9 | 257.3 | WebContent 54.0 · pitwall 36.5 · GPU 16.7 · audio.SandboxHelper 5.5 · Networking 4.8 · SetStoreUpdateService 2.4 |

## 읽는 법

- **콜드 스타트의 구성**: 띄운 프로세스(CLI)는 시작 후 5 – 8 ms 에 `main` 에 들어오고, 단일 인스턴스 확인과 Tauri 초기화를 거쳐 앱을 떼어 다시 띄운다. 떼어 낸 앱이 `main` 에 들어오기까지가 약 80 ms, 거기서 프로젝트 창의 첫 프레임까지가 약 390 ms 다. 떼어 띄우기(M0-10)가 콜드 스타트의 약 6분의 1 이다.
- **준비 회가 느린 이유**: 빌드 직후 첫 실행은 띄운 프로세스가 `main` 에 들어오는 데만 228 ms 가 걸렸다(이후 회는 5 – 8 ms). 새 바이너리를 처음 적재하는 비용이라 중앙값에서 뺐다.
- **메모리의 구성**: WebContent(웹뷰의 JS · DOM) 50 MiB 남짓, 앱 프로세스 36 MiB, GPU 17 MiB 가 대부분이다. `com.apple.audio.SandboxHelper` 와 `SetStoreUpdateService` 는 이름은 WebKit 이 아니지만 책임 프로세스가 그 앱 인스턴스라 함께 셌다(합쳐 8 MiB 남짓).
- **제외한 회**: 2회차는 첫 프레임 전에 문서가 `hidden` 이었다. 프레임이 3.2 s 늦었고 WebContent 도 46 MiB 로 다른 회보다 작다 — 가려진 창에서 WebKit 이 그리기와 메모리를 줄인 상태라 두 수치 모두 기준선이 아니다. 같은 이유로 이 기록 전의 시험 측정에서는 창이 46 s 동안 가려진 회가 나왔다(그 회도 GPU · WebContent 가 작았다). 그래서 하네스가 그런 회를 표시의 `hidden` 으로 가려 뺀다.
- **목표와의 거리**: 두 목표 모두 3분의 1 안팎이다. 지금 앱은 프로젝트 창에 도크 · 툴 윈도우 바 · 상태 표시줄만 있어, 에디터 · git · 파일 트리가 들어오면 다시 재 비교한다.

다시 재기: `mise run perf`. M0-16 처럼 프로젝트 창이나 `main.rs` 를 바꾸는 작업이 병합된 뒤에는 같은 명령으로 새 기록을 남긴다.
