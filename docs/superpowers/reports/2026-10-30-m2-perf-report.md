# M2 성능 확인 — M2-22

이 문서는 잰 시점의 기록이다. 측정 방법은 `CONTRIBUTING.md` 「성능 기준선」(「git 시나리오」 포함), 목표는 `docs/design/06-roadmap.md` 「성능 목표 (검증 기준)」, 완료 기준은 `docs/design/13-m2-plan.md` 의 M2-22 행이 정본이다. 비교 대상 M1 기준선은 [`2026-10-18-perf-baseline-report.md`](2026-10-18-perf-baseline-report.md).

**상태: 반쪽이다.** 이번 회차에서는 앱을 띄우지 않았다 — 앱이 필요한 칸은 「실기 대기」 이고 아무 값도 넣지 않았다. Rust 쪽 수치(아래 §2)만 잰 값이다. 실기 칸은 `mise run perf-m2` 한 번으로 채운다(§5, 절차는 `docs/ops/m2-realcheck.md`).

## 1. 조건

| 항목 | 값 |
| --- | --- |
| 커밋 | `d5999d8`(M2-21 까지 + 실기 측정 하네스) 위의 `jang2162/m2-22-perf`. Rust 측정 코드는 main 과 같다(이 브랜치가 바꾼 것은 `pw-perf` 결과에 `settings.project` 를 더한 것뿐) |
| 기기 | Mac17,9 · Apple M5 Pro · 15 CPU · 48 GiB, macOS 27.0.1 (26A434) — M1 기준선과 같은 기기 |
| 빌드 | `cargo build --release`(예제 · 시험), 같은 기기·같은 방법(각 행 완료 기준에 적힌 명령) |
| 부하 | **기기가 한가하지 않았다.** 다른 워크트리의 워커(vitest · cargo)가 도는 중이었다. 측정 시작 직전 load average(1/5/15분) 16.7/48.1/78.8, 끝날 때 12.8/45.2/77.0, 한 번 더 확인한 nextest 시험 시작 12.0 · 끝 11.6, 40만 커밋 측정 시작 15.0 · 끝 19.0. 코어 15개 대비 1분 평균 12~19 라 **수치는 부하 속의 값이다** — 한가한 기기에서는 더 작게 나올 수 있다. 앱이 필요한 측정(§3)은 기기를 비우고 돌린다 |
| 캐시 | 반복마다 저장소를 새로 연다. OS 파일 캐시는 데워진 채다(첫 반복이 가장 차갑다) |

## 2. 앱 없이 잰 Rust 수치 (이번에 다시 잼)

합성 저장소: `mise run perf-git-repos` 의 10만 커밋(`target/perf-git/graph` · `no-graph`, 실제 100,003 커밋), `synth_repo 100000 1 --git`(10만 파일), `synth_history 400000`(40만 커밋, commit-graph 없음).

| 항목 | 목표 | 이번 값(중앙값, 범위) | 이전에 보고된 값 | 증가분 · 판정 |
| --- | --- | --- | --- | --- |
| M2-03 `git.log` 첫 청크, 10만 커밋, commit-graph 있음 | ≤ 300 ms | **22.9 ms** (19.1 – 23.6, 예제 5회) · 시험 5회 23.5 – 24.7 ms | 합성 10만 110 ~ 240 ms 는 commit-graph 없음의 값(13 이월 표) | 목표 안 |
| M2-03 첫 청크, 10만 커밋, commit-graph 없음 | ≤ 300 ms | **114.6 ms** (97.3 – 132.2, 예제 5회) · 시험 5회 105 – 174 ms | 110 ~ 240 ms | 목표 안, 이전 범위 안 |
| M2-03 전체 배치(10만 행), 있음 / 없음 | 기록 | 303.3 ms (283 – 310) / 393.1 ms (381 – 445) | — | 기록만 |
| M2-03 최고 메모리(`/usr/bin/time -l`, 10만, 있음) | 기록 | 50.3 MB (maximum resident set size 50,282,496 B) | — | 기록만 |
| M2-03 스트림 크기(10만 행) | 기록 | 31.40 MB (첫 청크 62.1 KB) | — | 기록만 |
| M2-03 첫 청크, **40만 커밋, commit-graph 없음** | ≤ 300 ms | **447.4 ms** (354.6 – 564.0, 5회), 전체 1.78 s | 600 ms (13 이월 표) | **목표 초과**(300 ms). 06 의 500 ms 안에는 중앙값이 들지만 범위 위끝(564 ms)은 넘는다. §6 ① |
| M2-02 전체 status, 10만 파일(깨끗) | 기록 | 424.0 ms (355.9 – 754.5; 첫 호출 754.5) | — | 기록만. 같은 트리에서 `git status --porcelain=v2 -uall` 422.6 ms |
| M2-02 전체 status, 파일 하나 바뀜 | 기록 | 390.4 ms (381.0 – 438.1) | — | 기록만 |
| M2-02 경로 한정 status(`git.status` 경로, 캐시 있음 / 없음) | 기록 | 18.6 ms (18.4 – 68.5) / 68.4 ms (68.3 – 70.4) | — | 기록만. `git status -- <경로>` 19.0 ms |
| M2-02 `git.refs` ahead/behind, 10만 일직선 이력 | 기록 | commit-graph 없음 139.0 ms(센 세대 번호를 넘기면 0.51 ms), 있음 0.55 ms. `git rev-list` 9.3 / 9.7 ms | — | 기록만. 첫 호출이 commit-graph 없는 큰 저장소에서 139 ms |
| M2-05 blame, 5,000줄 · 커밋 1천 개 — gix | 판정(결정 M2-D1) | 첫 청크 = 전체 **392.9 / 398.6 ms** (388.7 – 415.6) | 약 0.5 s(13 이월 표) | 기록. gix 는 다 만든 뒤 돌려준다 |
| M2-05 같은 파일 — git CLI `--incremental -M` | 판정 | 첫 청크 **13.1 ms**, 전체 366.3 ms (346.2 – 383.0) | 11 ms(13 이월 표) | 신뢰한 프로젝트의 경로. 첫 청크는 gix 의 30분의 1 |
| M2-05 같은 파일 — git CLI `-M` 없이 | 참고 | 첫 청크 11.0 ms, 전체 185.3 ms | — | 참고 |
| M2-05 `git.fileHistory`(1천 커밋) | 기록 | 첫 1.2 ms, 전체 6.3 ms | — | 기록만 |

측정 명령(저장소 폴더는 `$S` 로 줄였다):

```bash
mise run perf-git-repos
cargo run --release -p pw-fs --example synth_repo -- $S/status-repo 100000 1 --git
target/release/examples/status_bench $S/status-repo 5
target/release/examples/log_timing target/perf-git/graph 5          # 있음; no-graph 도 같은 명령
/usr/bin/time -l target/release/examples/log_timing target/perf-git/graph 1   # 최고 메모리
cargo nextest run --release -p pw-git -j 1 --run-ignored only -E 'test(=log::tests::perf::the_first_chunk_of_100k_commits_is_within_300ms)' --no-capture   # 1 test run: 1 passed
target/release/examples/synth_history $S/h400k 400000 && target/release/examples/log_timing $S/h400k 5
target/release/examples/refs_bench $S/refs 100000 5
target/release/examples/blame_timing $S/blame 5
```

읽는 법:

- **commit-graph 없는 큰 저장소가 유일한 초과다.** 10만 커밋은 있음 23 ms / 없음 115 ms 로 넉넉하지만, 없음에서 첫 청크가 커밋 수에 비례해(10만 115 ms → 40만 447 ms, 약 3.9배) 40만에서 300 ms 를 넘는다. 이번 40만 값(447 ms)이 이월 표의 600 ms 보다 작은 것은 부하 · 저장소 차이일 수 있어 같은 조건이라는 뜻이 아니다 — 같은 기기에서 한 번 더 잴 때 하나로 맞춘다.
- **blame 의 gix 경로는 첫 청크가 곧 전체다**(393 ms). 06 에 blame 목표 수치는 없다. 미신뢰 프로젝트만 이 경로를 타고, 신뢰한 프로젝트는 git CLI 로 첫 청크 13 ms 다.
- **status 전체 424 ms 는 git CLI(423 ms)와 같다.** 파일 하나 바뀐 뒤 경로 한정 status 가 19 ms 라 창은 경로 한정으로 갱신한다.

## 3. 앱이 필요한 수치 — 실기 대기

M1 기준선은 [M1 보고서](2026-10-18-perf-baseline-report.md)(커밋 `235a7fd`, 임시 빈 프로젝트). 증가분 = 이번 값 − M1 기준선. 빈 칸은 **재지 않았다**.

| 항목 | 목표 | 이번 값 | M1 기준선 | 증가분 | 명령 |
| --- | --- | --- | --- | --- | --- |
| 콜드 스타트 → 프로젝트 창 첫 프레임, git 저장소 프로젝트(10만 커밋) | < 1500 ms | 실기 대기 | 440 ms (416 – 446, 5회) | 실기 대기 | `perf --project target/perf-git/graph` |
| 콜드 스타트 → 첫 프레임, 빈 프로젝트(M2 앱) | < 1500 ms | 실기 대기 | 440 ms | 실기 대기 | `mise run perf` |
| 유휴 메모리 `phys_footprint`, git 저장소 프로젝트 | < 300 MB | 실기 대기 | 123.2 MB (118.8 – 123.8) | 실기 대기 | `perf --project target/perf-git/graph` |
| 유휴 메모리, 빈 프로젝트(M2 앱) | < 300 MB | 실기 대기 | 123.2 MB | 실기 대기 | `mise run perf` |
| ⌘9 → Git 로그 첫 행 그림, commit-graph 있음 / 없음 | < 500 ms (M2-D10) | 실기 대기 / 실기 대기 | — (M2 에서 처음) | — | `perf-git` |
| 10만 행 스크롤, 프레임 ≤ 16.7 ms 몫 | ≥ 0.95 (60 fps) | 실기 대기 | — | — | `perf-git` |
| 1만 줄 diff 첫 그림 | < 200 ms | 실기 대기 | — | — | `perf-git` |
| 터미널 `git checkout` → 상태바 브랜치 | < 500 ms | 실기 대기 | — | — | `perf-git` |
| 한 줄 편집 → 거터 수정 표시 | < 100 ms | 실기 대기 | — | — | `perf-git` |
| 5,000줄 파일 30초 타이핑 중 가장 긴 메인 스레드 막힘(모든 회의 최댓값) | ≤ 50 ms | 실기 대기 | — | — | `perf-git` |
| 1만 파일 커밋 목록 스크롤, 프레임 ≤ 16.7 ms 몫 | ≥ 0.95 | 실기 대기 | — | — | `perf-git` |
| 터미널 `git add` → 스테이징됨 그룹 | < 500 ms | 실기 대기 | — | — | `perf-git` |
| 시나리오 직후 메모리(참고, 유휴 기준선 아님) | 기록 | 실기 대기 | — | — | `perf-git` |

M2-14 의 「그래프 첫 화면 · 스크롤 5회」 는 `perf-git` 의 저장소 둘 × 측정 5회가 그 5회다(`CONTRIBUTING.md` 「git 시나리오」).

앱 없이 본 것: 헤드리스 WebKit 의 10만 행 스크롤(`apps/desktop/perf/git-log`)은 M2-14 에서 이미 돌았다 — release 번들의 값이 아니므로 위 칸을 채우지 않는다.

## 4. 측정 명령 · 하네스

- `mise run perf-m2`(= `node scripts/perf-m2.mjs`): ① `mise run perf-git`(저장소 둘 × 준비 1 + 측정 5) ② `mise run perf -- --project target/perf-git/graph`(콜드 스타트 · 유휴 메모리를 10만 커밋 저장소 프로젝트로)를 차례로 돌리고, 각 `result.json` · `summary.md` 를 `docs/superpowers/reports/<날짜>-m2-perf-git.json|.summary.md` · `…-graph-project.json|.summary.md` 로 복사한다. `--short` 는 둘 다 1회(파일 이름에 `-short`). 화면이 잠겨 있으면 시작하지 않는다. OS 키 입력은 보내지 않는다.
- 하네스 변경: `pw-perf` 의 `result.json` 에 `settings.project`(`--project` 로 연 폴더)를 남기고 `summary.md` 에 `- project:` 줄을 더했다 — 없으면 이 결과가 빈 프로젝트의 것인지 10만 커밋 저장소의 것인지 구별되지 않는다. `--project` 는 M2-22 준비 때 이미 들어 있었다.
- 이 값들을 채울 때 이 문서 §3 의 칸에 옮기고 증가분을 계산한다. 기준선을 갱신할 때는 커밋하지 않은 변경이 없는 트리에서 잰다(`commit.dirty` 가 false).

## 5. 사람이 한 번에 돌리는 절차

`docs/ops/m2-realcheck.md` 「1. 측정」 이 정본이다(짧게 한 번 → 전체, 걸리는 시간, 앱이 앞으로 뜨는 횟수, 키 입력 없음, 결과 위치).

## 6. 목표를 넘는 항목 — M3 착수 전 대응 행 제안

지금 확인된 초과는 하나다. 실기 칸이 채워지면 초과가 더 나올 수 있고, 그때 같은 방식으로 이 절에 더한다(대응 행은 13 문서 이월 표의 「M2-22 수치를 보고」 두 행이 후보를 이미 갖고 있다).

| # | 초과 항목 | 대응 행 제안(M3 착수 전) | 근거 |
| --- | --- | --- | --- |
| ① | **commit-graph 가 없는 큰 저장소의 첫 화면**(40만 커밋 · 없음: 첫 청크 447 ms, 범위 355 – 564 ms, 목표 300 ms; 10만은 115 ms 로 안) | **M2-23 (제안) Git 로그 첫 청크를 커밋 수와 상관없게** — 순서: ⒜ commit-graph 쓰기 제안(사람이 누르는 쓰기, 결정 M2-D10 대안 ①: 첫 청크가 300 ms 를 넘은 저장소에서만 상태바/빈 영역에 「commit-graph 만들기」, `git commit-graph write --reachable` 을 M2-06 실행기로, 연결 범위 `Project(그 프로젝트)`, 확인 필요) ⒝ 필요하면 git 처럼 커밋 시각으로 미리 내다 늦은 커밋이 오면 다시 보내는 점진 순서. 완료 기준: 40만 커밋 · 없음에서 첫 청크 ≤ 300 ms(⒝) 또는 ⒜ 뒤 ≤ 50 ms | 13 이월 표 「commit-graph 가 없는 큰 저장소에서 첫 화면을 커밋 수와 상관없게 하기」 |
| ② | (참고, 목표 없음) 미신뢰 프로젝트 blame 첫 청크 393 ms(gix 는 다 만든 뒤 돌려준다; 신뢰한 프로젝트는 13 ms) | 대응 행을 만들지 않는다 — 13 이월 표의 「gix-blame 이 점진 결과를 낼 때」 를 그대로 둔다. 큰 파일에서 눈에 띄면 그때 `git.blame` 의 첫 청크에 「계산 중」 표시를 먼저 보내는 것을 검토 | 06 에 blame 목표 수치가 없고 신뢰한 프로젝트가 주 사용 경로 |
| ③ | (실기 대기) 「커밋 상세의 바뀐 파일 목록만 받기」 | `perf-git` 의 커밋 상세가 아니라 1만 파일 커밋 목록 스크롤 · diff 첫 그림이 목표를 넘을 때만 `git.diff` 에 hunk 를 뺀 선택 입력을 더한다 | 13 이월 표 |
