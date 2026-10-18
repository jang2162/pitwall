# 성능 기준선 — M0-20

이 문서는 잰 시점의 기록이다. 지금의 측정 방법은 `CONTRIBUTING.md` 「성능 기준선」 절, 목표는 `docs/design/06-roadmap.md` 「성능 목표 (검증 기준)」 절이 정본이다. 기계가 읽는 원본은 같은 이름의 `2026-10-18-perf-baseline-report.json`(`pw-perf` 의 `result.json` 그대로)이다.

## 조건

| 항목 | 값 |
| --- | --- |
| 명령 | `mise run perf -- --out target/perf/baseline` (종료 코드 0) |
| 빌드 | release 번들 `target/release/bundle/macos/Pitwall.app` (태스크가 그 자리에서 다시 만든 것) |
| 커밋 | `e3502c47cc0b4e192df42067ef480b940d9c5c74`, 커밋하지 않은 변경 없음 |
| 기기 | Mac17,9 · Apple M5 Pro · 15 CPU · 48 GiB, macOS 27.0.1 (26A434) |
| 반복 | 준비 1회 + 측정 5회(5회 모두 유효), 유휴 = 첫 프레임 10초 뒤 |
| 캐시 | warm 디스크 캐시의 새 프로세스(회 사이에 `purge` 없음) |
| 단위 | MB = 10^6 바이트(06 의 300MB 와 같은 단위) |
| 격리 | 측정 앱은 회마다 기본이 아닌 `$DATA` 라 단일 인스턴스 키(`dev.pitwall.desktop.data-<해시>`)와 웹뷰 저장소가 회마다 따로였다. 사람의 `~/Library/WebKit/dev.pitwall.desktop` 기본 저장소는 측정 전후 파일 수정 시각 · SHA-256 이 같았다 |
| 같은 시각의 환경 | 사람이 기기를 쓰는 중이었고, 다른 워크트리(M0-16)의 debug 앱이 떠 있었다 |

## 결과

| 지표 | 중앙값 | 범위 | 목표 |
| --- | --- | --- | --- |
| 콜드 스타트 → 프로젝트 창 첫 프레임 | 432 ms | 415 – 460 ms (5회) | < 1500 ms — 목표 안 |
| 유휴 메모리(`phys_footprint`, 앱 + WebKit) | 126.5 MB | 126.2 – 126.8 MB (5회) | < 300 MB — 목표 안 |
| 유휴 메모리(RSS, 같은 프로세스, 참고) | 270.8 MB | 270.4 – 271.4 MB | |
| 시작 → 떼어 띄운 앱이 `main` 에 들어온 때 | 84 ms | 73 – 95 ms | |

| 회 | 콜드 스타트 (ms) | 앱 main (ms) | footprint (MB) | RSS (MB) | 프로세스별 footprint (MB) |
| --- | --- | --- | --- | --- | --- |
| 0 (준비) | 646 | 285 | 122.0 | 270.9 | WebContent 52.8 · pitwall 38.2 · GPU 17.7 · audio.SandboxHelper 5.8 · Networking 5.0 · SetStoreUpdateService 2.5 |
| 1 | 439 | 76 | 126.2 | 270.4 | WebContent 52.3 · pitwall 42.9 · GPU 17.6 · audio.SandboxHelper 5.8 · Networking 5.1 · SetStoreUpdateService 2.5 |
| 2 | 432 | 89 | 126.5 | 270.8 | WebContent 52.7 · pitwall 43.0 · GPU 17.4 · audio.SandboxHelper 5.8 · Networking 5.1 · SetStoreUpdateService 2.5 |
| 3 | 415 | 73 | 126.8 | 271.4 | WebContent 52.9 · pitwall 42.9 · GPU 17.6 · audio.SandboxHelper 5.8 · Networking 5.1 · SetStoreUpdateService 2.5 |
| 4 | 460 | 95 | 126.8 | 271.1 | WebContent 52.5 · pitwall 43.5 · GPU 17.5 · audio.SandboxHelper 5.8 · Networking 5.0 · SetStoreUpdateService 2.4 |
| 5 | 425 | 84 | 126.2 | 270.5 | WebContent 52.3 · pitwall 43.0 · GPU 17.6 · audio.SandboxHelper 5.8 · Networking 5.0 · SetStoreUpdateService 2.5 |

## 읽는 법

- **콜드 스타트의 구성**: 띄운 프로세스(CLI)는 시작 후 5 – 8 ms 에 `main` 에 들어오고, 단일 인스턴스 확인과 Tauri 초기화를 거쳐 앱을 떼어 다시 띄운다. 떼어 낸 앱이 `main` 에 들어오기까지가 약 85 ms, 거기서 프로젝트 창의 첫 프레임까지가 약 350 ms 다. 떼어 띄우기(M0-10)가 콜드 스타트의 약 5분의 1 이다.
- **준비 회가 느린 이유**: 빌드 직후 첫 실행은 띄운 프로세스가 `main` 에 들어오는 데만 207 ms 가 걸렸다(이후 회는 5 – 8 ms). 새 바이너리를 처음 적재하는 비용이라 중앙값에서 뺐다.
- **메모리의 구성**: WebContent(웹뷰의 JS · DOM) 약 52.5 MB, 앱 프로세스 약 43 MB, GPU 약 17.5 MB 가 대부분이다. `com.apple.audio.SandboxHelper` 와 `SetStoreUpdateService` 는 이름은 WebKit 이 아니지만 책임 프로세스가 그 앱 인스턴스라 함께 셌다(합쳐 약 8.3 MB).
- **측정 앱과 정식 앱의 차이(원인 미확인)**: 측정 앱은 기본이 아닌 `$DATA` 로 떠서 회마다 따로 만든 웹뷰 저장소(`data_store_identifier`)를 쓴다. 정식 앱은 기본 저장소를 쓴다. 저장소를 나누기 전 같은 기기의 시험 측정에서는 앱 프로세스가 약 38 MB 였는데 이번 측정에서는 약 43 MB 다. 따로 만든 저장소가 그 차이의 후보다 — 정식 앱의 메모리는 이 값보다 몇 MB 작을 수 있다.
- **제외 규칙**: 첫 프레임 전이든 뒤든 유휴 표본 전에 창이 가려지면 그 회를 뺀다. 이 기록 직전의 시도(같은 코드, 마지막 커밋 하나 전)에서는 측정 5회 중 3회가 첫 프레임 0.6 – 0.7 s 뒤에 한 번 가려져 빠졌고, 유효한 회가 2회라 pw-perf 가 경고와 함께 3 으로 끝났다. 그 뒤의 1회 탐색에서는 Pitwall 이 띄운 지 0.19 s 에 맨 앞 앱이 되어 끝까지 앞에 있었고 가려지지 않았다. 원인(사람이 뜬 창에서 돌아간 것인지)은 확인하지 못했다.
- **목표와의 거리**: 콜드 스타트는 목표의 3분의 1 안팎, 메모리는 목표의 5분의 2 남짓이다. 지금 앱은 프로젝트 창에 도크 · 툴 윈도우 바 · 상태 표시줄만 있어, 에디터 · git · 파일 트리가 들어오면 다시 재 비교한다.

다시 재기: `mise run perf`. M0-16 처럼 프로젝트 창이나 `main.rs` 를 바꾸는 작업이 병합된 뒤에는 같은 명령으로 새 기록을 남긴다.
