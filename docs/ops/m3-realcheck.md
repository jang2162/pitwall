# M3 실기 측정 (M3-21)

사람이 기기를 비워 줄 때 한 번에 돌릴 것. 대상은 14-m3-plan.md M3-21 행이 release 번들에서 재는 것이다 — 결과 그리드(M3-16), 스키마 탐색기(M3-13), Redis 키 브라우저(M3-18), 콜드 스타트 · 유휴 메모리(DB 연결 하나를 연 프로젝트). 측정이 무엇을 어떻게 재는지는 `CONTRIBUTING.md` 「성능 기준선」 의 「DB 시나리오」 절이 정본이다 — 이 문서는 그 절차를 한 번 돌리는 운영 메모다. Rust 쪽 수치(M3-05 · M3-06 · M3-09)는 앱 없이 각 행 완료 기준의 시험으로 잰다(아래 3절).

## 1. 돌리기 전에

1. **화면이 잠기지 않았는지** 본다: `ioreg -n Root -d1 -a | grep -A1 CGSSessionScreenIsLocked` 가 `<false/>` 이거나 아무것도 안 나와야 한다. 잠겨 있으면 스크립트가 그 단계를 시작하지 않고 멈춘다.
2. **창이 뜰 화면과 그 주사율**: 스크립트가 `system_profiler SPDisplaysDataType` 를 조건 파일에 남긴다. 덮개 · 전원 · 외부 모니터 여부는 사람이 보고서에 적는다 — 스크롤 판정은 간격 중앙값 17 ms 이하를 요구한다.
3. **「화면 보호기 시작」 을 「안 함」 이나 1시간 이상으로** 둔다. pw-perf 가 측정 동안 `caffeinate` 로 잠자기는 막지만 화면 보호기 뒤의 잠금까지 막는지는 확인하지 않았다.
4. **도커(colima 등)가 떠 있어야 한다.** 스크립트는 자기 이름의 컨테이너 둘(`pitwall-perf-<워크트리 해시>-pg|redis`)만 만들고 끈다 — 다른 컨테이너는 건드리지 않는다. 이미지는 CI `db-live` 와 같다(`scripts/db-tls.mjs` 의 `PG_IMAGE` · `REDIS_IMAGE`, 없으면 받는다).
5. 측정 중에는 **기기를 쓰지 않는다.** 프로젝트 창이 회마다 한 번 앞에 뜬다 — 가리면 그 회는 제외된다(`hidden`).

## 2. 명령

| 단계 | 명령 | 걸리는 시간 | 앱이 앞으로 뜨는 횟수 |
| --- | --- | --- | --- |
| 1. 짧게 한 번 | `mise run perf-m3 -- --short` (단계마다 `--runs 1 --warmup 0`) | 번들 두 벌 빌드(코드가 바뀌었으면 각 5~10분) + 측정 5~8분 | **5번** |
| 2. 전체 | `mise run perf-m3` | 번들 빌드 + 측정(빈 프로젝트 둘 각 2분, DB 유휴 2분, DB 시나리오 둘 각 6회) | 6 × 5 = **30번** |

- 1 이 끝까지 돌고(**종료 코드 0** — 3 은 유효한 회가 적다는 뜻이라 통과가 아니다, 요약에 실패 회 없음) 2 로 간다. 실패한 단계만 다시 볼 때는 `--only db-fake,db-pg` 처럼 고른다(번들 둘은 늘 만들고 견준다).
- 하는 일: ① PG(10만 행 × 10열 `perf_rows` · 그 뷰 `perf_stream`) · Redis(키 100만 개)를 빈 포트에 띄우고 심는다 ② 기본 번들과 `perf-fake-db` 번들을 만들어 `target/release/perf-m3/<default|fake>/` 로 옮기고, 기본 번들에 가짜 코드가 없는지 본다 ③ pw-perf 다섯 단계: `empty-default` · `empty-fake`(빈 프로젝트 콜드 스타트 · 유휴 메모리 — 두 번들의 차이), `db-idle`(기본 번들, PG 연결 하나를 콘솔에서 연 뒤 유휴 메모리), `db-fake`(가짜 번들 — 가짜 postgres 그리드 · 탐색기 + Redis), `db-pg`(기본 번들 — 실 PG 그리드) ④ 결과를 보고서 폴더로 복사 ⑤ 컨테이너 둘을 끈다(`--keep-db` 면 남긴다).
- 키 입력: **OS 키 입력을 보내지 않는다.** ⌘8 · ⌘⇧L 은 웹뷰 안의 키 이벤트, [연결] · [실행] · ▸ 는 그 요소에 보낸 클릭이다. 측정 중 화면이 잠겨도 잠금 화면에 글자가 들어가지 않는다(잠긴 회는 멈춰 시간 제한에 걸리고 실패로 적힌다).
- 단계 사이: 시작 직전마다 화면 잠금을 보고, 1분 load average 가 코어 수를 넘으면 내려갈 때까지 15초마다 기다린다(기다린 초가 조건 파일에 남는다).
- 앱 고르기 · 끄기: pw-perf 가 띄운 pid 와 그 책임 프로세스 묶음으로만 고르고 그 pid 만 끈다. 끝나면 `pgrep -fl 'perf-m3/.*/Pitwall.app'` 가 비어야 한다.

## 3. Rust 쪽 수치 (앱 없이)

PG · Redis 는 위 컨테이너(`--keep-db`)나 따로 띄운 것을 쓴다. 회마다 아래를 5번 돌려 중앙값을 적는다.

```bash
cargo nextest run --release -p pw-db -j 1 --run-ignored all --no-tests=fail -E 'test(=first_rows_chunk_of_100k_arrives_within_50ms)' --no-capture   # M3-05 ①: 「first rows chunk: worst …」(한 번에 5번 재고 가장 느린 것)
PW_TEST_PG_URL=postgres://postgres@127.0.0.1:<port>/<db> cargo nextest run --release -p pw-db-driver-pg -j 1 --no-tests=fail -E 'test(/live_pg_the_first_page_of_ten_thousand_tables_is_fast/)' --no-capture   # M3-06 ⑦: 「first page of 10,000 tables: …」
PW_TEST_REDIS_URL=redis://127.0.0.1:<port>/0 cargo nextest run --release -p pw-db-driver-redis -j 1 --no-tests=fail -E 'test(/live_redis_the_first_scan_batch_of_a_million_keys_is_fast/)' --no-capture   # M3-09 ①: 「first scan batch: … best of 5 …」
PW_TEST_REDIS_URL=redis://127.0.0.1:<port>/0 cargo nextest run --release -p pw-db -j 1 --no-tests=fail -E 'test(/live_redis_the_first_scan_chunk_of_a_million_keys_is_fast/)' --no-capture   # M3-22 ⑤: Host 스트림의 첫 청크(창 첫 화면의 Host 몫)
```

`Summary … 1 test run: 1 passed` 를 읽는다 — 0개면 URL 변수가 빠져 시험이 목록에 없는 것이다(`crates/pw-db-driver/live_cfg.rs`). Redis 시험은 그 DB 를 비우고 다시 심으므로 측정용 Redis 와 따로 띄우거나 측정 뒤에 돌린다.

## 4. 끝나면

- 결과: `docs/superpowers/reports/<날짜>-m3-perf-<종류>[-short]-<시각>.json|.summary.md` 다섯 쌍과 `…-m3-perf-conditions[-short]-<시각>.json`(커밋 · 번들 판정 · 단계별 load · 기다린 시간 · 화면). 시각이 붙어 다시 돌려도 덮지 않는다. `--short` 결과는 커밋하지 않는다(확인용).
- 보고서: `docs/superpowers/reports/2026-11-01-m3-perf-report.md` 의 수치표에 중앙값 · 범위 · 회수를 옮기고, M2 기준선(`2026-10-30-m2-perf-report.md` §3)과의 증가분을 적는다. 목표를 넘은 항목마다 M4 착수 전 대응 행을 제안한다.
- 측정 기준선은 커밋하지 않은 변경이 없는 트리에서 잰다(결과의 `commit.dirty` 가 false).
