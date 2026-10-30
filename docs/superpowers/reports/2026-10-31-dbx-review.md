# DBX 코드 검토 — M3(DB) 착수 전 근거 자료

이 문서는 검토한 시점의 기록이다. 무엇을 만들지의 정본은 `docs/design/02-tech-stack.md` 「DB 계층: DBX 구조 참고」 절, `docs/design/04-features.md` 「F6. 데이터베이스」, `docs/design/06-roadmap.md` 의 M3 행 · 결정 D8 · D15 다. **이 문서는 M3 계획이나 설계를 정하지 않는다** — 정해야 할 것은 끝의 「M3 계획 전에 사람이 정할 질문」 에 모았다.

## 검토 조건

| 항목 | 값 |
| --- | --- |
| 대상 | https://github.com/t8y2/dbx, `git clone --depth 1` 로 받은 커밋 `c5e7e0430653aaef198800a48f9dec46a7cfed5e`(커밋 시각 2026-10-31T03:08:40+08:00) |
| 받은 코드의 취급 | 미신뢰. 읽기만 했다 — 그 폴더에서 빌드 · 스크립트 · 인터프리터를 돌리지 않았다 |
| 규모 | `crates/` 와 `src-tauri/` 의 `.rs` 합계 775,805줄(시험 포함, `find crates src-tauri -name '*.rs' \| xargs cat \| wc -l`). 큰 파일: `crates/dbx-driver-postgres/src/postgres.rs` 17,602 · `crates/dbx-core/src/connection/mod.rs` 15,934 · `crates/dbx-driver-redis/src/lib.rs` 12,724 · `crates/dbx-driver-mysql/src/mysql.rs` 11,988 · 프런트엔드 `apps/desktop/src/components/grid/DataGrid.vue` 18,599 |
| 읽은 방법 | 드라이버 · SSH · 자격 증명 / SQL 위험도 · 안전장치 / 쿼리 · 스키마 · 그리드 / MCP · CLI · JDBC · 시험의 네 갈래로 나눠 읽고, 결론을 좌우하는 주장(공통 드라이버 trait 없음, SQLite 워커의 용도, 위험도 enum · 파싱 실패 처리, read-only 판정 함수, MCP 의 비밀번호 인자, 에이전트 jar 체크섬, CI 러너, 결과 DTO · 행 스트리밍 여부, 그리드 기본 모드)은 원본에서 다시 확인했다 |
| 크레이트 메타데이터 | crates.io API(`/api/v1/crates/<이름>`)의 최신 안정판 · 공개일 · 라이선스. 정책 검사는 scratch 의 프로브 매니페스트에 이 저장소의 `deny.toml` 을 두고 `cargo-deny check` 를 돌렸다(빌드 없음 — `cargo metadata` 수준). 다시 재는 법은 끝의 「다시 재는 법」 |

아래에서 DBX 경로는 위 커밋 기준이고, 줄번호 대신 함수 · 타입 이름으로 가리킨다.

## 요약

**가장 큰 발견 둘 — 02 의 전제와 다르다.**

1. **DBX 는 sqlx 를 쓰지 않는다.** PG 는 `tokio-postgres` + `deadpool-postgres`(루트 `Cargo.toml` 의 `[patch.crates-io]` 가 GaussDB 포크 git 저장소로 바꿔 끼운다), MySQL 은 `mysql_async`(호환 패치 포크 git), SQLite 는 `rusqlite`, Redis 는 `redis` 1.7, SSH 는 `russh` 0.60 이다. 02 「결론」 표의 DB 행(「sqlx + redis-rs」)과 09 의 DBX 행(「`sqlx`/`tiberius`/`redis-rs`」)은 DBX 를 근거로 삼을 수 없다.
2. **sqlx 의 SQLite 드라이버는 지금 Pitwall 과 함께 링크되지 않는다.** `sqlx-sqlite` 0.9.0 은 `libsqlite3-sys >=0.30.1, <0.38.0` 을 요구하고 Pitwall 은 `rusqlite` 0.40.2 → `libsqlite3-sys` 0.38.2 를 쓴다. `links = "sqlite3"` 는 그래프에 하나만 허용되므로 해석이 실패한다(「다시 재는 법」 프로브 c 에서 재현).

### 가져올 것(설계 · 코드 차용 후보)

- **SQL 위험도 분류의 AST 부분** — `crates/dbx-sql-core/src/sql_risk.rs` 의 `classify_statement` · `query_is_write_capable` · `predicate_is_obviously_unbounded`(WHERE 가 항진식인 UPDATE/DELETE 탐지)와 그 시험 34개. 의존은 `sqlparser`(Apache-2.0)와 `DatabaseType` 뿐이라 떼어 올 수 있다. 단 파싱 실패 처리 등 결함은 고쳐서 가져와야 한다(「SQL 위험도 분류」).
- **실행 예산 · 취소 모델** — `crates/dbx-driver-support/src/execution.rs` 의 `DbOperationBudget`(연결 · 체크아웃 · 재활용 · 취소 5 s · 정리 3 s 타임아웃 묶음), 「행이 올 때마다 다시 감는」 비활동 타임아웃(`await_stream_with_progress_timeout`), 실행 id 별 `CancellationToken` + 드라이버별 인터럽트 클로저 등록(`crates/dbx-core/src/query/query_cancel.rs` 의 `RunningQueries` · `register_interrupt`).
- **SSH 호스트 키 TOFU 검증기** — `crates/dbx-driver-support/src/ssh_host_key.rs` 의 `HostKeyVerifier`(186줄): `~/.ssh/known_hosts` 를 먼저 보고(바뀐 키는 거절, 그 파일에 쓰지 않음) 앱 전용 known_hosts 로 TOFU, 프롬프트가 없으면 거절.
- **값 → JSON 규칙** — `crates/dbx-types/src/json.rs`(91줄)의 JS 안전 정수(±2^53−1 밖은 문자열)와 결과 직렬화의 `JsSafeCell`, PG `timestamptz` 를 텍스트 프로토콜로 읽는 이유(바이너리 디코더가 호스트 시간대를 적용) — `pg_type_requires_text_protocol`.
- **Redis 명령 콘솔의 허용 목록 분류** — `crates/dbx-driver-redis/src/lib.rs` 의 `classify_command`(`Allowed` / `Write` / `Confirm` / `Blocked`, 모르는 명령은 `Blocked`)와 따옴표 인식 토크나이저 `parse_command_argv`.

### 참고만 할 것

- 크레이트 분할(타입 · SQL · 드라이버 지원 · 드라이버별 · core)과 「하위 크레이트는 core 를 역참조하지 않는다」 규칙(`crates/ARCHITECTURE.md`).
- 스키마 탐색: PG 는 `pg_catalog` 위주 + OID 배열 일괄 조회(`get_columns_for_relations` 등), 테이블 목록은 서버 쪽 필터 + LIMIT/OFFSET, 트리는 노드 펼칠 때 지연 로드, PG DDL 은 `pg_get_*def` 조각으로 합성(`pg_ddl`), MySQL 은 `SHOW CREATE TABLE`.
- 문장 나누기: `crates/dbx-sql-core/src/sql.rs` 의 `SqlStatementSplitter`(방언 프로필 플래그로 달러 인용 · `DELIMITER` · 루틴 블록 · `GO` 처리).
- 그리드 편집의 행 식별: PK → PG 는 `tableoid` · `ctid` · `xmin` 숨은 열 → 그 밖은 전 열 WHERE + 「정확히 1행」 가드(`crates/dbx-sql-data/src/data_grid_sql.rs`).
- Redis 키 브라우저: `SCAN` + 1000키 단위 `TYPE`·`TTL` 파이프라인, 컬렉션은 4 MiB 페이지 예산의 `HSCAN`/`SSCAN`/`ZRANGE`/`LRANGE`.
- 수동 트랜잭션: 세션마다 연결을 고정하고(`TxnConnection`) 300 s 쉬면 롤백.
- MCP: 툴별 허용 목록(글롭), 고위험 SQL 은 그 SQL 문자열에 묶인 1회용 승인 토큰.
- JDBC agent: 별도 프로세스 · stdin/stdout JSON-RPC 2.0 · 핸드셰이크로 프로토콜 판본 협상(F6-3b 는 P2, 참고만).

### 다르게 할 것

- **결과를 통째로 돌려주지 않는다.** DBX 는 `QueryResult { rows: Vec<Vec<serde_json::Value>>, … }` 를 invoke 응답으로 한 번에 돌려주고(쿼리 명령에 `Channel` 사용 0건) 페이징은 SQL 에 LIMIT 를 덧씌워 다시 실행한다. Pitwall 은 03 「RPC 설계」 의 스트림(credit 백프레셔)이 정본이다.
- **read-only 를 클라이언트 판정에만 맡기지 않는다.** DBX 데스크톱의 read-only 연결은 키워드 판정(`check_read_only` → `is_write_sql_for_database`)만으로 막아 `SELECT pg_terminate_backend(…)` · `SELECT setval(…)` · `SELECT … FOR UPDATE` 가 지나간다. DB 수준 읽기 전용(`BEGIN READ ONLY` 등)은 EXPLAIN ANALYZE 와 백업 스냅숏에만 쓴다.
- **분류기를 둘 두지 않는다.** DBX 는 Rust(`sql_risk.rs`)와 TS(`apps/desktop/src/lib/sql/sqlRisk.ts`, 480줄 키워드 토크나이저)가 따로 있고 결과가 갈린다 — TS 쪽은 `COPY … TO PROGRAM` 을 read 로 본다. Pitwall 은 Host 의 분류 하나를 RPC 로 UI · MCP 가 함께 쓴다(02 「RPC 계약 우선」).
- **MCP 툴이 비밀번호를 받지 않는다.** DBX `dbx_add_connection` 의 `AddConnectionRequest` 에는 `pub password: String` 이 있다. Pitwall 은 05 의 `db_connection_upsert` 규칙(비밀은 UI 입력 또는 env 참조)대로 간다.
- **SQLite 「격리 워커」 는 DBX 에 없는 것을 우리가 새로 정하는 일이다.** 02 는 DBX 가 SQLite 파일을 격리 워커에서 연다고 적었지만, `dbx-sqlite-worker` 는 **SSH 원격 호스트의 SQLite 파일을 읽으려고 원격에 올려 돌리는 바이너리**다(`Cargo.toml` description: 「Short-lived SQLite worker process for remote file-host access over SSH」, 쓰는 조건 `sqlite_remote_worker_requested` = SQLite 이면서 전송 계층이 있을 때). 로컬 파일은 메인 프로세스의 `rusqlite` 로 연다. Pitwall 의 원격 파일은 Relay 가 그 머신에서 직접 열므로(08) 업로드 워커는 필요 없다.
- **드라이버 trait 를 만든다면 DBX 에서 베낄 것이 없다.** DBX 에는 공통 드라이버 trait 가 없고 약 30개 변형의 `enum PoolKind`(`crates/dbx-core/src/connection/mod.rs`)를 연산마다 `match` 한다. 메타데이터도 드라이버별 자유 함수다.
- **외부 다운로드는 서명으로 검증한다.** DBX 의 에이전트 레지스트리는 jar 의 `sha256` 이 `Option`(`agent_manager.rs` 의 `ArtifactInfo`)이고 빈 값이면 검사를 건너뛰며, 크기 · 해시가 같은 레지스트리 파일에서 와 독립된 신뢰 근거가 없다.
- **TLS 기본값을 따로 정한다.** DBX PG 는 libpq 의 `sslmode` 의미를 따른다 — `prefer` · `require` 는 인증서를 검증하지 않고(`postgres_sslmode_accepts_invalid_certs`), `prefer` 는 TLS 실패 시 평문으로 다시 붙는다. MySQL 은 URL 이 요구하지 않으면 도메인 검증을 건너뛴다. 결함이 아니라 libpq 호환 선택이지만 Pitwall 기본값은 사람이 정할 일이다(질문 10).

## 02 「DB 계층: DBX 구조 참고」 분할안과 다른 점

| 02 의 서술 | 실제 DBX(c5e7e04) | 보고서의 판단 |
| --- | --- | --- |
| DBX 는 Rust `sqlx`/`tiberius`/`redis-rs` | `tokio-postgres`(GaussDB 포크 패치) · `deadpool-postgres` · `mysql_async`(포크 패치) · `rusqlite` · `redis` · `tiberius`(vendor 패치). `sqlx` 없음 | 02 결론 표의 「sqlx」 는 DBX 와 무관한 독립 결정이 된다 — 유지할지 질문 1 |
| `pw-db-types` ← `dbx-types` | 맞다. 연결 설정 · DB 식별 · 쿼리/메타데이터 DTO · JS 안전 JSON | 대응 유지 가능 |
| `pw-db-sql` ← `dbx-sql` | `dbx-sql` 은 433줄의 파사드이고 실체는 `dbx-sql-core`(파싱 · 위험도 · 문장 나누기) · `dbx-sql-data`(그리드 저장 DML · 페이징 SQL) · `dbx-sql-dialect`(방언 파일 로드 · 감시) · `dbx-sql-schema`(스키마 비교 · DDL) 넷 | 한 크레이트로 둘지는 크기에 달렸다. M3 범위(위험도 + 문장 나누기 + 페이징/편집 DML)면 하나로 충분해 보인다 |
| `pw-db-driver` ← `dbx-drivers`, `dbx-driver-support` — 「드라이버 trait」 | 공통 trait 없음. `dbx-driver-support` 는 실행 예산 · SSH 호스트 키 · ssh config 같은 공용 부품, `dbx-drivers` 는 드라이버 크레이트를 모듈로 재수출하고 SSH 터널 · SQLite · JDBC agent 를 가진다. 분기는 `dbx-core` 의 `enum PoolKind` | trait 로 갈지 enum 디스패치로 갈지 질문 2 |
| `pw-db-driver-{postgres,mysql,sqlite,redis}` ← `dbx-driver-*` | `dbx-driver-{postgres,mysql,redis,sqlserver,mongodb,elasticsearch,agent}` 는 있고 **SQLite 전용 크레이트는 없다**(`dbx-drivers/src/db/sqlite.rs`) | SQLite 는 Pitwall 이 이미 `rusqlite` 를 링크하므로 분리 이득이 작다 — 질문 3 |
| `pw-db` ← `dbx-core` | `dbx-core` 는 34만 줄짜리 앱 오케스트레이션(연결 · 쿼리 · 스키마 · 데이터 이전 · AI · 관리 콘솔 · 저장 · 안전 · 업데이트) | 역할은 대응하지만 크기 · 범위는 비교 대상이 아니다 |
| SQLite 파일은 격리 워커에서 연다(손상 파일 · 긴 쿼리가 메인을 막지 않게) | 워커는 SSH 원격 전용. 로컬은 `spawn_blocking` + `std::sync::Mutex<rusqlite::Connection>`, 취소는 `interrupt_handle` | 「격리」 는 DBX 근거가 없는 새 결정 — 질문 4 |
| DBX 도 MCP 서버와 CLI 를 같은 코어 위에 둔다 | 맞다. `dbx-mcp` · `dbx-cli` · `dbx-web` · 데스크톱이 모두 `dbx-core` 에 의존. 다만 CLI 는 MCP 정책(`McpGlobalPolicy`)을 읽지 않는 별도 쓰기 판정을 가진다 | 원칙은 일치. Pitwall 은 판정 경로를 하나로 |
| (언급 없음) | 크레이트 수 27(워크스페이스 멤버), 공용 부품 `dbx-platform`, 플러그인 런타임 · AI 제공자 크레이트 | M3 범위 밖 |

## 기능별 정리

판정: **가져옴** = 설계 또는 코드를 차용할 후보, **참고** = 방식만 참고, **다르게** = Pitwall 원칙과 맞지 않아 다른 방식이 필요.

| 기능 | DBX 가 하는 방식(경로 · 함수) | 쓰는 크레이트 | 판정 · 이유 |
| --- | --- | --- | --- |
| 드라이버 추상 | `enum PoolKind`(약 30 변형) + `ConnectionPoolRegistry`(`HashMap<String, PoolKind>`, `AppState` 안 `Arc<RwLock<…>>`). 연산마다 `match`(예: `query/mod.rs` 의 `do_execute_typed`) | — | **다르게**. 드라이버 4개 + 이후 확장(MSSQL · JDBC 확장)을 생각하면 trait 이든 enum 이든 Pitwall 이 정해야 한다 |
| PG 드라이버 | 풀 `deadpool-postgres`(`connect_postgres_pool_attempt`, `RecyclingMethod::Fast`, 공유 풀 10 · 세션 풀 1 — `postgres_pool_max_connections_for_session`). 스트리밍은 `query_raw`, 미지원 타입은 `simple_query_raw` 폴백. 취소는 와이어 CancelRequest(`cancel_postgres_query`, 풀 키별 `PostgresCancelContext`). keepalive 주입. TLS 는 rustls(aws-lc) 기본 + openssl 레거시 커넥터 | tokio-postgres(포크) · deadpool-postgres · tokio-postgres-rustls · postgres-openssl · openssl(vendored) | **참고**. 포크 패치는 `deny.toml` 의 `sources`(`unknown-git = "deny"`)에 걸리므로 그대로 못 쓴다. openssl vendored 는 C 빌드를 끌어온다 |
| MySQL 드라이버 | `mysql_async::Pool`(`PoolConstraints::new(1, max)`, 문장 캐시 0, keepalive). 취소는 새 연결로 `KILL QUERY <id>`(`kill_query_with_opts`) | mysql_async(포크) | **참고**. 공개판 `mysql_async` 0.37.1 의 rustls 기능은 `webpki-roots`(CDLA-Permissive-2.0)를 끌어와 `deny.toml` 에 걸린다(「크레이트 후보」) |
| SQLite | `SqliteHandle { conn: Option<Arc<std::sync::Mutex<rusqlite::Connection>>>, worker }`, 풀 없음, `spawn_blocking`, `busy_timeout` 10 s, 취소 `interrupt_handle`. 확장 로드는 기능 게이트 + `unsafe LoadExtensionGuard` | rusqlite 0.32 | **참고**. 인터럽트 핸들을 얻으려 async 스레드에서 std 뮤텍스를 잡는다(바쁜 연결이면 tokio 워커를 막을 수 있음 — 읽어서 추론, 재현 안 함). Pitwall 은 미신뢰 프로젝트의 SQLite 파일 · 확장 로드 규칙을 따로 정해야 한다(질문 13) |
| Redis | `RedisConnection::Direct(Mutex<MultiplexedConnection>)` · `Cluster`. DB 번호는 연산마다 `SELECT`. 키 브라우저 `scan_keys_batch_inner`(SCAN + DBSIZE + 1000키 TYPE·TTL 파이프라인, 클러스터는 커서 상위 16비트에 노드 번호). 타입별 뷰어 `get_value_scoped_with_preview_limit`(string · list · set · zset · hash · stream · JSON · bitmap · HLL, 4 MiB 페이지 예산, 긴 문자열은 `GETRANGE` 창). TTL `set_ttl` · `set_expire_at` · 해시 필드 TTL. Sentinel 은 직접 구현(`connect_sentinel_with_timeout`). **응답 타임아웃 없음**(`set_response_timeout(None)`), **취소 없음** | redis 1.7.1(tokio-comp · tls-rustls · sentinel · cluster-async) | 콘솔 분류는 **가져옴**, 나머지는 **참고**. redis-rs 의 `aio` 기능이 `xxhash-rust`(BSL-1.0)를 무조건 끌어와 `deny.toml` 에 걸린다 |
| 실행 제한 · 타임아웃 | `MAX_ROWS = 10000`, `QUERY_TIMEOUT = 30 s`, 연결 타임아웃 기본 5 s(1..300 으로 자름), 쿼리 타임아웃은 행마다 다시 감는 비활동 타이머 | — | **가져옴**(구조). 비활동 타이머는 「긴 스트림 전체」 를 자르지 않는다는 점을 의도로 적어야 한다 |
| 스키마 탐색 | 명령 `crates/dbx-tauri-schema/src/commands.rs`(`list_databases` · `list_schemas` · `list_tables`(filter · limit · offset) · `get_columns` · `list_indexes` · `list_foreign_keys` · `get_table_ddl`). PG 는 `pg_catalog` 위주, OID 배열 일괄 조회, 호환 쿼리 단계 폴백. MySQL 은 `information_schema` → `SHOW` 폴백. 트리 지연 로드 + 「더 보기」 페이지. 캐시는 앱 SQLite 에 저장(`save_schema_cache`) + `object_cache.rs` 가 DDL · COMMIT/ROLLBACK 뒤 무효화 | — | **참고**. 큰 스키마 대응(서버 필터 + 페이지 + 일괄 조회)은 그대로 쓸 만한 방식. 쿼리 본문 차용은 고지 대상 |
| 콘솔 | `execute_multi` 가 서버에서 나눠 문장마다 `ExecuteMultiResult`(UI 결과 탭). `continue_on_error` · `use_transaction`. 탭마다 `clientSessionId` 로 세션 풀(연결 1개) 고정. 수동 커밋은 `begin_manual_transaction` → `txn_session_id`, 300 s 쉬면 롤백. 진행은 이벤트 `query-batch-progress` · `query-messages` | — | **참고**. 「탭 = 고정 연결」 과 「쉬면 롤백」 은 F6-5 · F6-9 수동 커밋에 맞다. 세션 수명은 Pitwall 의 연결 범위(03 「연결 범위」)와 묶어야 한다 |
| 결과 그리드(전송 · 페이징) | 통째 반환(`QueryResult`, `truncated` · `has_more`). 행 상한: 백엔드 기본 10,000, 프런트엔드 기본 100,000. PG · MySQL 은 N행에서 읽기를 멈춤(서버 커서 없음). 페이징은 `build_paginated_query_sql`(방언별 LIMIT/OFFSET · OFFSET FETCH · ROWNUM). 큰 셀은 미리보기 + `value_ref` 지연 읽기(`read_large_value_chunk`), 데이터 탭 32 MB 바이트 상한 | — | **다르게**(전송) · **참고**(큰 셀 지연 읽기). PG 에서 N행 뒤 끊을 때 서버가 나머지를 보내는지 · 연결을 버리는지는 확인하지 못했다(위험 · 모르는 것) |
| 타입 → 표시 변환 | PG `classify_pg_type` → `pg_value_to_json_classified`: numeric · money · int8(범위 밖) · uuid · inet · interval → 문자열, bytea → `"0x…"`, json/jsonb → 문자열(파싱 안 함), 배열 → JSON 배열, geometry → WKT, timestamptz → 텍스트 프로토콜. MySQL BIGINT · DECIMAL 은 항상 문자열. SQLite BLOB 은 hex(TEXT 친화 + UTF-8 이면 문자열) | — | **가져옴**(규칙). 「정밀도를 잃는 숫자는 문자열」 은 TS 쪽 표현과 함께 RPC 타입으로 고정할 것 |
| 그리드 UI | 서드파티 그리드 없음. 자체 canvas 렌더러가 기본(`dataGridRenderMode: "canvas"`, `canvasDataGridRenderer.ts`, 행 26 px, 스크롤 높이 16,000,000 px 로 자르고 가상 매핑), DOM 모드는 `vue-virtual-scroller` ^3.0.4 | — | **참고**. Pitwall 은 Glide Data Grid(02). 1,600만 px 상한 처리처럼 수십만 행에서 부딪히는 지점은 Glide 에서도 확인할 것 |
| 그리드 편집(F6-8, P1) | `prepare_data_grid_save` → `DataGridSavePreparation { statements, rollback_statements, keyless_guards, bound_statements }`. 미리보기 대화상자 → 한 트랜잭션(`execute_statements_in_transaction`) 또는 수동 트랜잭션 | — | **참고**. 되돌리기 SQL 을 같이 만드는 점, PK 없는 테이블의 가드 방식 |
| 자동완성(F6-6) | CodeMirror 6 `@codemirror/lang-sql` + 스키마 캐시 명령(`list_completion_objects`), 분석은 웹 워커 | — | **참고** |
| 내보내기(F6-11) | 전체 결과는 Rust 가 다시 페이지로 실행하며 임시 파일에 스트리밍(`export_query_result_core_with_completion`), 인코더 `crates/dbx-formats`(자체 XLSX 작성기) | — | **참고** |
| 히스토리(F6-10) | 앱 SQLite `history` 표(`Storage::save_history_entry`), MCP 호출도 같은 표 | — | **참고** |
| SQL 위험도 분류(F6-9) | 아래 「SQL 위험도 분류」 | sqlparser 0.62(visitor) · winnow | **가져옴**(AST 부분, 고쳐서) |
| 안전장치(F6-9) | 아래 「안전장치」 | — | **다르게**(DB 수준 강제를 더한다) |
| SSH 터널(F6-1) | 아래 「SSH 터널」 | russh 0.60 · pageant · portpicker | 검증기 **가져옴**, 구현 방식은 질문 11 |
| 자격 증명 저장 | 아래 「자격 증명」 | keyring · AES-256-GCM · argon2 | **참고** |
| JDBC agent(F6-3b) | 아래 「JDBC agent」 | Java 21 · Gradle shadow · HikariCP | **참고만**(P2) |
| MCP · CLI | 아래 「MCP · CLI」 | rmcp 3.5.1 · axum | **참고**, 비밀번호 인자 · 정책 이중화는 **다르게** |
| 실 DB 시험 | 아래 「시험 방식」 | — | **다르게**(Pitwall CI 는 macOS 포함) |

### SQL 위험도 분류

- **자리**: `crates/dbx-sql-core/src/sql_risk.rs`(2,337줄 — 분류기와 WHERE 항진식 분석 약 920줄, 읽기 증명 · 함수 허용 목록 약 540줄, 시험 약 870줄 34개).
- **등급**: `enum SqlRisk { ReadOnly, Write, Ddl, Transaction }` 에 별도 불리언 `is_dangerous_sql_for_database`(범위 없는 DML). F6-9 의 다섯 갈래와는 이렇게 대응한다 — 조회 = `ReadOnly`, 쓰기 = `Write`, WHERE 없는 UPDATE·DELETE = `Write` + dangerous, DDL = `Ddl`, **DROP·TRUNCATE 도 `Ddl`**(따로 갈리지 않음). `GRANT`/`REVOKE` 는 `Ddl`.
- **알고리즘**: `classify_statement` 가 sqlparser `Statement` 를 `match`. `Query` 는 `query_is_write_capable` 이 CTE 안의 DML · 잠금절 · `SELECT INTO` · 부작용 함수 목록(`SIDE_EFFECT_SELECT_FUNCTIONS`: `nextval` · `setval` · `pg_terminate_backend` · advisory lock · `lo_*` · `pg_reload_conf` 등)을 본다. `EXPLAIN ANALYZE X` 는 X 로, `COPY` · `PRAGMA` 는 항상 `Write`, **나머지는 `_ => SqlRisk::Write`**. 토큰 검사 둘이 곁들여진다(`FOR UPDATE/SHARE` 등 잠금절, MySQL `/*! */` 실행 주석 · `INTO OUTFILE`).
- **방언**: 연결의 `DatabaseType` → Postgres · MySQL · SQLite · MsSql · ClickHouse · DuckDB, 그 밖은 Generic.
- **여러 문장**: 문장별 등급의 최댓값(`risk as u8`). `Transaction` 이 마지막 변형이라 `BEGIN; DROP …` 은 `Ddl` 이 아니라 `Transaction` 이 된다.
- **파싱 실패**: 키워드 폴백(`query_execution_sql.rs` 의 `is_write_sql_for_database` — 주석 · 리터럴을 걷고 첫 키워드가 SELECT/WITH/SHOW/DESC/EXPLAIN/FROM 인지 + 위험 동사 검색). 원본 주석 스스로 「keyword fallback on parse failure may answer ReadOnly」 라고 적는다 — **읽기처럼 보이는 파싱 불가 문장은 fail-open**.
- **더 엄격한 두 번째 API**: `prove_read_only_for_database` → `ReadProof::{ProvenReadOnly, Unproven}`(문장 하나 + 순수 함수 허용 목록, 파싱 실패는 `Unproven`). DBX 는 「UI 전용, 보안 경계 아님」 으로 표시하고 수동 트랜잭션의 커밋 버튼 표시에만 쓴다.
- **잘 다루는 것**(시험 있음): 주석에 숨긴 `DROP`, 쓰기 가능한 CTE(중첩 포함), `SELECT … INTO`, `pg_terminate_backend`/`nextval`/`setval`, `COPY … TO PROGRAM`(Write + dangerous), `EXPLAIN ANALYZE DELETE`, `FOR UPDATE`, `UPDATE … WHERE 1=1`(dangerous — `predicate_is_obviously_unbounded` 가 상수 · `id=id` · `x IS NULL OR x IS NOT NULL` · `LIKE '%'` · 열 없는 술어 · **모든 서브쿼리** 를 무한정으로 본다).
- **틈**: ① 목록 밖 부작용 함수(`pg_sleep` · `dblink_exec` · `set_config` · `lo_export` · MySQL `SLEEP`/`GET_LOCK` · 사용자 정의 함수)는 `ReadOnly` ② 파싱 실패 fail-open ③ `SET` · `LOCK` · `VACUUM` · `DO $$…$$` · `LOAD DATA` · `ATTACH` 는 catch-all 이나 폴백으로 `Write` 가 되지만 전용 시험이 없다 ④ `PRAGMA` 는 AST 경로에서 언제나 Write, 키워드 경로는 안전한 pragma 8개를 허용해 두 경로가 갈린다 ⑤ 프런트엔드 TS 분류기와 불일치.
- **Redis 명령**: 별도 분류기 `classify_command`(`RedisCommandSafety { Allowed, Write, Confirm, Blocked }`). 읽기는 허용 목록, `DEL` · `RENAME` · pop 류는 `Confirm`, `FLUSHALL` · `FLUSHDB` · `KEYS` · `CONFIG` · `EVAL` · `FCALL` · `SHUTDOWN` · 모르는 명령은 `Blocked`, `MONITOR` 거절. 데스크톱은 `skip_safety_check` 로 넘길 수 있다.
- **떼어 오기**: AST 분류기 · 항진식 분석 · 잠금 토큰 검사는 자체 완결. 키워드 폴백은 `query_execution_sql.rs`(3,341줄 중 관련 약 1,000줄)와 문장 나누기(`sql.rs` 8,587줄)에 엮여 있어 같이 가져오거나 fail-closed 로 바꿔 대체해야 한다.

### 안전장치

- **read-only 연결**(`ConnectionConfig.read_only`): 클라이언트 판정만. 데스크톱 경로는 키워드 판정(`check_read_only_for_connection` → `check_read_only`)이고, MCP 만 AST 판정과 키워드 판정을 OR 한다. `default_transaction_read_only` · `SET TRANSACTION READ ONLY` 류를 사용자 연결에 걸지 않는다. DB 수준 읽기 전용은 `QueryExecutionMode::PostgresReadOnlyTransaction`(`BEGIN READ ONLY` 후 항상 롤백 — EXPLAIN ANALYZE 용)과 백업 스냅숏뿐.
- **쓰기 잠금 해제**: `crates/dbx-core/src/safety/write_unlock.rs` 의 `WriteUnlockWindows` — 저장된 플래그를 바꾸지 않고 (자격 소유자, 연결) 마다 60 s / 300 s 창을 연다. MCP · CLI 는 무시.
- **프로덕션 태그**(`is_production`, `production_databases`): 데스크톱은 프런트엔드에서 쓰기 문장마다 확인(저장 · 끄기 불가) + 툴바 색 배지. 백엔드의 교차 DB 대상 탐지(`targets_production_database`, 정규식)는 MCP · CLI · 웹 MCP 만 부르고, 거기서 프로덕션 쓰기는 **차단**.
- **위험 SQL 확인(데스크톱)**: `useSqlExecution.ts` 의 `DANGER_RE = /^\s*(DROP|DELETE|TRUNCATE|ALTER|UPDATE|MERGE|REPLACE)\b/i` — WHERE 를 보지 않고 설정으로 끌 수 있다. 백엔드 분류와 별개다.
- **수동 커밋**: 탭별 `autoCommit`(DB 종류별 기본값), 백엔드 트랜잭션 세션 300 s 유휴 롤백(연결별 조정 가능).
- **Pitwall 과 맞추기**: 05 「권한 모델」 의 `dangerous` 등급(DB 쓰기 · 프로덕션 연결 접근: 매번 승인, 프로덕션 기본 차단)과 DBX MCP 의 동작은 거의 같다. 다른 것은 DBX 데스크톱 경로가 같은 판정을 쓰지 않는다는 점이다.

### SSH 터널

- **자리**: `crates/dbx-drivers/src/db/ssh_tunnel.rs`(4,845줄), russh 0.60.
- **호스트 키**: `check_server_key` 가 `HostKeyVerifier` 로 `Trusted` / `Unknown`(프롬프트) / `Changed`(이전 지문과 함께 프롬프트) / 오류(거절)를 가른다. `~/.ssh/known_hosts` 에서 바뀐 키는 무조건 거절, 앱 전용 known_hosts 에서 바뀐 키는 교체 또는 이번 세션만 신뢰를 묻는다. 프롬프트 300 s 시간 초과, 프롬프트 게이트웨이가 없으면 거절(fail-closed). known_hosts 를 `std::fs` 로 async 콜백 안에서 읽고 쓴다.
- **인증**: none → 비밀번호 → 키 파일(`decode_secret_key` — OpenSSH/PEM/PKCS8 의 RSA · Ed25519 · ECDSA, passphrase 는 비밀 저장소) → keyboard-interactive → 에이전트(unix `AgentClient::connect_env` 또는 지정 소켓, Windows 는 OpenSSH 파이프 · Pageant, 지문으로 신원 선택, 인증서 신원 지원).
- **알고리즘**: 기본 목록 뒤에 SHA-1 KEX · HMAC-SHA1 · AES-CBC 를 폴백으로 붙인다. keepalive 30 s.
- **포워딩**: `portpicker::pick_unused_port()` 로 고른 포트에 `127.0.0.1`(옵션으로 `0.0.0.0`) 리스너 → 연결마다 `channel_open_direct_tcpip` + `copy_bidirectional`. SOCKS5 모드도 있다. 점프 호스트는 홉마다 세션 + 로컬 리스너(`start_chain`), `~/.ssh/config` · ProxyCommand 해석(`dbx-driver-support/src/ssh_config.rs`, `ssh_proxy_command.rs`).
- **수명**: `tunnel_reconnect_loop` 5 s → 60 s 지수 백오프 최대 10회, 30 s 핑, `TunnelStatus` 보고.
- **Pitwall 에 걸리는 것**: ① `portpicker` 0.1.1 은 Unlicense(허용 목록 밖) · 마지막 릴리스 2021-06-18 — 포트 0 바인드로 대체 가능 ② russh 기본 기능 `rsa` 는 `rsa` 크레이트를 끌어오고 RUSTSEC-2023-0071(Marvin, 고친 판본 없음)이 `cargo deny check advisories` 를 실패시킨다(「크레이트 후보」) ③ 08 「연결 방식」 은 Relay 접속에 시스템 `ssh` 바이너리를 쓰기로 했다(사용자 설정 · 에이전트 · ProxyJump · ControlMaster 존중). DB 터널도 같은 원칙을 따를지 질문 11.

### 자격 증명

- 연결 설정과 비밀은 앱 SQLite `dbx.db` 의 `connections(id, config_json)` · `connection_secrets(connection_id, key, secret, secret_enc)`(`crates/dbx-core/src/persistence/storage.rs`). 새 값은 `secret_enc` 에 `dbxenc1.<nonce>.<ct>`(AES-256-GCM, `secret_codec.rs`).
- 암호 키 하나만 OS 키체인(`keyring`, 서비스 `com.dbx.app.secret-store.v1`; Linux 는 secret-service)에 두고, 헤드리스는 `DBX_SECRET_KEY` / `DBX_SECRET_KEY_FILE`, 없으면 키 파일. 패스프레이즈 유도는 Argon2id.
- `save_password=false` 면 저장소에서 지우고 메모리(`SessionCredentialStore`)에만 둔다.
- MCP 서버 프로세스는 데스크톱의 `dbx.db` 를 직접 열고(`DBX_DATA_DIR` 로 바꿈) 같은 키체인 키를 쓴다.
- **Pitwall 과의 차이**: 03 「데이터 저장」 예시는 연결 설정을 프로젝트의 `.pitwall/db/<이름>.toml` 에 두고 `password = { secret_ref = "keychain" }` 로 가리킨다(설정 공유 · 비밀 비공유). 08 은 원격 Relay 에 「필요한 것만 세션 동안」 비밀을 넘긴다. DBX 의 「키 하나 + 암호문 DB」 는 키체인 프롬프트 수를 줄이는 장점이 있다 — 어느 쪽인지 질문 12.

### JDBC agent(F6-3b, 참고만)

- Java(JDK 21, Gradle shadow, HikariCP 포함) · Go · Rust 에이전트를 별도 프로세스로 띄우고 stdin/stdout JSON-RPC 2.0 으로 말한다(`crates/dbx-driver-agent/src/agent_driver.rs` 의 `spawn_agent_process`, 프록시 환경 변수 제거).
- 핸드셰이크 `{"appVersion", "supportedProtocolVersions":[2]}` → `protocolVersion` · `capabilities`(`multi_session` · `structured_error_v1` · `transaction` 등), 모르면 v1 폴백. 명세 `agents/docs/agent-protocol-v2.md`(세션 열기 · 검증 · 취소 · 닫기, 오류의 `sessionDisposition`: keep / quarantine / replace_runtime).
- 런타임 하나가 논리 세션 최대 256개, 마지막 세션이 닫히면 30 s 유예 뒤 종료, EOF 면 대기 요청을 실패시키고 다음 요청에 다시 띄운다.
- JRE 21 을 자동 내려받고(없으면 `JAVA_HOME` → `PATH`), 드라이버 jar 는 GitHub Release 의 `agent-registry.json`(R2 · CNB 미러)에서 받는다. **서명 검증 없음, sha256 선택**.
- 라이선스 주의: 저장소에 라이선스 파일 없이 커밋된 벤더 JDBC jar 가 7곳(`agents/drivers/{cache,gbase8a,gbase8c,oscar,sundb,transwarp,uxdb}/libs/*.jar`), Maven 으로 받는 드라이버 중 Jaybird(보통 LGPL) · `mysql-connector-j`(보통 GPL + FOSS 예외) · H2(보통 MPL/EPL)가 있다(저장소 안에서 라이선스는 확인하지 않음). 이 방식을 차용하면 jar 를 앱이 배포하지 않고 사용자가 받게 하는 경계가 라이선스상 중요하다.

### MCP · CLI

- `dbx-mcp`: rmcp 3.5.1, stdio 기본 + Streamable HTTP(`127.0.0.1:5225/mcp`, Bearer 토큰 필수 · 상수 시간 비교 · Host/Origin 검사, 루프백 밖 바인드는 환경 변수와 플래그 둘 다 필요). `#[tool]` 50개 — 연결 · 스키마 · SQL(`dbx_execute_query` 기본 100행, 최대 1000) · 세션/트랜잭션 · Redis · MQ · UI 연동 · Salesforce · Nacos · Consul · 플러그인.
- 쓰기 관문(`validate_sql_policy` 류): `USE` 금지 → `Transaction` 차단 → 쓰기 = AST 등급 ≠ ReadOnly OR 키워드 판정 → 정책 read-only · 연결 read-only 차단 → `Ddl` 또는 dangerous 는 `allow_dangerous_sql` 필요 → 프로덕션 쓰기 차단. 고위험 SQL 은 데스크톱 승인 → 그 SQL 에 묶인 1회용 토큰.
- 데스크톱 승인 브리지는 `127.0.0.1:0` 의 HTTP 이고 **Bearer 토큰이 없다** — Origin 헤더가 있거나 JSON 이 아니거나 Host 가 루프백이 아니면 거절하는 것이 방어의 전부다(`src-tauri/src/commands/mcp_bridge.rs` 의 `bridge_request_is_local_client`). Pitwall 은 사용자 전용 폴더(0700) · 소켓 0600 · uid 검사(03 「보안」 의 git askpass 소켓 규칙)를 이미 쓴다.
- 아무 정책도 저장하지 않은 새 설치의 실효 기본값은 「DDL 없는 쓰기 허용」 으로 읽혔다(`McpGlobalPolicy` 가 `Default` 를 derive — `read_only = false`). 실행해서 확인하지는 않았다. Pitwall 05 는 `db_query` 기본이 읽기 전용 트랜잭션이다.
- 감사: 호출마다 `HistoryEntry`(요청 · 응답 16 KiB, `password` · `passwd` · `secret` 키 가림). 툴 허용 목록 `allowed_tool_names`(글롭, `TOOL_OUT_OF_SCOPE`).
- `dbx-cli`: `doctor` · `capabilities` · `agent setup|status` · `connections list` · `schema list|describe` · `query`(`--limit` · `--timeout` · `--allow-writes` · `--allow-dangerous-sql`) · `context` · `dbml` · `docs` · `open`. `dbx-mcp` 의 백엔드를 재사용하지만 쓰기 판정은 MCP 정책을 읽지 않는 별도 경로.
- `dbx-web`: 앱 전체를 HTTP/SSE/WebSocket 으로 내놓는 axum 서버, 기본 `0.0.0.0:4224`, 단일 비밀번호(Argon2) 쿠키 세션, `DBX_DISABLE_PASSWORD=1` 로 끌 수 있다. Pitwall 의 원격 모델(Relay + Noise, 08)과는 다른 갈래라 참고 대상이 아니다.

### 시험 방식

- `crates/ARCHITECTURE.md` 「验证」 절: 로컬에서 DB 엔진 · Docker 를 띄우지 않는다. 단위 시험은 인메모리 SQLite 와 로컬 프로토콜 모의(예: 결함 주입 MySQL 와이어 프록시 `crates/dbx-mcp/tests/support/mysql_wire_proxy.rs`). 실 DB 는 SSH 시험 서버의 전용 인스턴스에 SSH 터널로 붙는 `#[ignore]` live 시험(약 379개)이고, 「돌리지 않은 live 시험은 통과로 치지 않는다」.
- live 시험 환경 변수: PG `DBX_TEST_POSTGRES_URL` · `DBX_LIVE_POSTGRES_{HOST,PORT,USER,PASSWORD,DATABASE}`, MySQL `DBX_LIVE_MYSQL_URL` · `DBX_LIVE_MYSQL57_URL` 등, Redis `DBX_TEST_REDIS_URL` · `DBX_LIVE_REDIS_PORT`. SQLite 전용 live 변수는 없다(인메모리로 시험).
- testcontainers 없음(`Cargo.lock` 에 0건). CI(`.github/workflows/ci.yml`)의 시험 잡은 `ubuntu-24.04`(일부 Windows)이고 **macOS 시험 러너가 없다**. `services:` 컨테이너도 쓰지 않는다. `deploy/database/<엔진>/<판>/compose.yaml`(PG 14/17 · MySQL 5.7/8.4 · Redis 등)은 사람이 `make db*` 로 띄우는 실험실이고 CI 는 그 도구만 검사한다.
- **Pitwall CI 에서 쓸 수 있는 후보**(이 저장소 `.github/workflows/ci.yml` 의 `check` 잡은 ubuntu · macos · windows 행렬):
  1. 드라이버 독립 로직(위험도 분류 · 문장 나누기 · 값 변환 · DML 생성)은 DB 없이 단위 시험 — 세 OS 모두.
  2. SQLite 는 `rusqlite` 인메모리 · 임시 파일 — 세 OS 모두.
  3. PG · MySQL · Redis 실 DB 시험은 ubuntu 잡에만 `services:` 컨테이너(또는 testcontainers). GitHub 호스팅 macOS 러너에서는 Docker 를 쓸 수 없는 것으로 알려져 있다(이번에 재지 않음 — 확인 필요).
  4. macOS 잡에서도 실 DB 가 필요하면 Homebrew 로 `postgresql` · `mysql` · `redis` 를 설치해 띄우는 방법(설치 시간 미측정).
  5. DBX 식 `#[ignore]` live 시험 + 환경 변수 — 사람의 로컬 · 별도 서버용. 0개 실행을 통과로 읽지 않는 규칙은 이미 있다(`CONTRIBUTING.md` 「명령」).

## 크레이트 후보

공개일은 crates.io 최신 안정판의 공개일이다. 「deny.toml」 열은 아래 프로브로 이 저장소의 `deny.toml` 에 비춘 결과다.

| 크레이트 | 최신 안정판 · 공개일 | 라이선스 | DBX 사용 | deny.toml 에 비춘 결과 · 비고 | 대안 |
| --- | --- | --- | --- | --- | --- |
| `sqlx`(postgres · mysql) | 0.9.0 · 2026-05-21 | MIT OR Apache-2.0 | 안 씀 | `tls-rustls-ring-webpki` 로는 `webpki-roots`(CDLA-Permissive-2.0) 거절. advisories 통과 | 네이티브 드라이버 |
| `sqlx-sqlite` | 0.9.0 · 2026-05-21 | MIT OR Apache-2.0 | 안 씀 | **Pitwall 의 `rusqlite` 0.40 과 `links = "sqlite3"` 충돌로 해석 불가** | `rusqlite`(이미 씀) |
| `tokio-postgres` | 0.7.18 · 2026-06-12 | MIT OR Apache-2.0 | 씀(GaussDB 포크 패치) | 공개판은 통과. 포크 git 은 `sources` 거절 | `sqlx-postgres` |
| `deadpool-postgres` | 0.14.2 · 2026-08-26 | MIT OR Apache-2.0 | 씀 | 통과 | `bb8` 0.9.1(MIT, 2025-11-24) |
| `tokio-postgres-rustls` | 0.14.0 · 2026-05-21 | MIT | 씀(0.13) | 통과 | `postgres-openssl`(C 빌드) |
| `mysql_async` | 0.37.1 · 2026-09-01 | MIT OR Apache-2.0 | 씀(포크 패치) | `default-rustls` 는 `rustls-tls` → `webpki-roots` 거절, 암호 공급자는 aws-lc-rs(`default-rustls-ring` 으로 ring 가능) | `sqlx-mysql`(같은 webpki 문제), `native-tls-tls` 기능 |
| `mysql_common` | 0.38.2 · 2026-07-24 | MIT OR Apache-2.0 | 간접 | 통과 | — |
| `rusqlite` | 0.41.1 · 2026-10-28 | MIT | 씀(0.32) | Pitwall 은 0.40.2 사용 중 | — |
| `redis` | 1.8.1 · 2026-10-30 | BSD-3-Clause | 씀(1.7.1) | `aio` 기능이 `xxhash-rust` 0.8.19(BSL-1.0)를 무조건 끌어와 거절 | `fred` 10.1.0(MIT, 마지막 릴리스 2025-02-27), `deadpool-redis` 0.23.1(풀만) |
| `sqlparser` | 0.63.0 · 2026-09-13 | Apache-2.0 | 씀(0.62) | 통과 | `pg_query` 18.1.0(MIT 크레이트, libpg_query C 빌드, PG 전용) |
| `winnow` | 1.0.4 · 2026-07-13 | MIT | 씀 | 통과 | — |
| `russh` | 0.65.0 · 2026-10-25 | Apache-2.0 | 씀(0.60) | 기본 기능 `aws-lc-rs` · `rsa` · `flate2`. `ring` + `rsa` 로 프로브 → licenses 통과, **advisories 실패: RUSTSEC-2023-0071**(`rsa` 0.10.0-rc.18, 고친 판본 없음) | 시스템 `ssh -L`, `ssh2` 0.9.6(libssh2 C · OpenSSL) |
| `russh-keys` | 0.49.2 · 2024-12-20 | Apache-2.0 | 안 씀 | russh 본체로 합쳐져 갱신 멈춤 | — |
| `pageant` | 0.2.5 · 2026-10-25 | Apache-2.0 | 씀(Windows) | — | — |
| `portpicker` | 0.1.1 · 2021-06-18 | Unlicense | 씀 | 허용 목록 밖, 5년 넘게 릴리스 없음 | 포트 0 바인드 후 `local_addr()` |
| `keyring` | 4.2.0 · 2026-08-29 | MIT OR Apache-2.0 | 씀 | 기본 기능(`v1`)으로 통과. 4.x 는 `keyring-core` + 플랫폼별 저장소 크레이트로 나뉨 | `security-framework` 3.7.0 직접(macOS) |
| `aws-lc-rs` / `aws-lc-sys` | 1.19.0 / 0.47.0 · 2026-10-28 | ISC AND (Apache-2.0 OR ISC) 외 허용형 조합 | 씀(rustls 공급자) | 통과. Pitwall 은 지금 `ring` 0.17.14 를 쓴다 — 둘 다 들이면 공급자가 둘 | `ring` |
| `openssl` / `openssl-src` | 0.10.81 / 400.0.2+4.0.3 | Apache-2.0 / MIT/Apache-2.0 | 씀(vendored, 레거시 TLS) | 프로브에 넣지 않음. C 빌드 | rustls |
| `rust_decimal` | 1.43.0 · 2026-09-02 | MIT | 씀 | 통과 | `bigdecimal` 0.4.11 |
| `testcontainers` / `testcontainers-modules` | 0.28.0 / 0.15.0 | MIT OR Apache-2.0 / MIT | 안 씀 | modules 0.15 는 `testcontainers` 0.27 을 요구(0.28 과 함께 해석 불가). 0.27 로 프로브 → 통과 | `services:` 컨테이너 |
| `tiberius`(MSSQL, P2) | 0.13.0 · 2026-09-25 | MIT/Apache-2.0 | 씀(0.12.3, vendor 패치) | 프로브에 넣지 않음 | — |

**정리**: 어느 드라이버 조합을 고르든 `deny.toml` 을 고치지 않고는 통과하지 않는다. 걸리는 것은 셋이다 — ① `webpki-roots`(CDLA-Permissive-2.0, MySQL rustls · sqlx rustls-webpki) ② `xxhash-rust`(BSL-1.0, redis-rs aio) ③ RUSTSEC-2023-0071(russh 의 `rsa` 기능). 지금 `deny.toml` 에는 같은 라이선스의 크레이트 단위 예외가 이미 있다(`webpki-root-certs` 의 CDLA-Permissive-2.0 — wasm32 전용이라 배포 바이너리에 안 들어간다는 조건, `clipboard-win` · `error-code` 의 BSL-1.0). 이번 것들은 배포 바이너리에 들어가므로 같은 근거로 넣을 수 없다 — 질문 14.

## 라이선스 — 차용할 때의 고지

DBX 는 Apache-2.0(루트 `LICENSE`)이고 `NOTICE` 는 PL/SQL 디버거 기여(Ziyu Yang, OceanBase ODC 에서 각색)만 적는다. `.rs` 파일에 라이선스 머리말은 없다. `vendor/` 의 8개는 모두 MIT · Apache-2.0 계열이다(`pageant` · `rumqttc` 는 라이선스 파일 없이 `Cargo.toml` 에 Apache-2.0).

DBX 코드를 Pitwall 에 옮겨 넣을 때 필요한 것(Apache-2.0 4절, `CONTRIBUTING.md` 「의존성과 라이선스」, 06 「서드파티 코드 차용」 행):

1. **`licenses/vendored.md` 에 항목을 손으로 쓴다** — 원본 이름(DBX) · 출처 URL과 커밋(`c5e7e0430653aaef198800a48f9dec46a7cfed5e`) · 옮긴 원본 파일 경로와 Pitwall 쪽 경로 · 라이선스(Apache-2.0) · Apache-2.0 전문 · 변경 사실(「Rust 로 옮기며 무엇을 바꿨다」 — 기존 Linguist 항목의 꼴). 그리고 `mise run notices` 로 `THIRD_PARTY_NOTICES.md` 를 다시 만들어 함께 커밋한다.
2. **DBX `NOTICE` 중 옮긴 부분에 해당하는 고지를 싣는다** — Apache-2.0 4(d)는 원본 `NOTICE` 의 고지를 파생물에 넣되 「파생물의 어느 부분에도 해당하지 않는 고지」 는 빼도 된다고 한다. 그 커밋의 `NOTICE` 는 PL/SQL 디버거 기여에 관한 것뿐이라, 위험도 분류 · 드라이버 · SSH 코드를 옮기는 한 실을 문장이 없다. 차용할 때마다 그 시점의 `NOTICE` 를 다시 보고 판단한다.
3. **옮긴 파일에 변경 표기를 남긴다** — 4(b) 「수정한 파일에는 변경했다는 눈에 띄는 고지」. 파일 머리 주석에 출처(DBX 경로 · 커밋)와 「수정함」 을 쓴다. DBX 파일에는 머리말이 없으니 「유지할 머리말」 은 없다.
4. **방식만 참고하고 코드를 옮기지 않은 것은 고지 대상이 아니다.** 이 보고서의 「참고」 판정이 여기다. 「가져옴」 은 코드 · 시험 케이스를 옮길 때만 위 1~3 이 필요하다 — 시험 케이스(SQL 문자열 목록)를 그대로 가져오는 것도 옮기는 것이다.
5. **의존성으로 쓰는 크레이트**는 생성기가 고지를 채운다(`mise run notices`). 위 「크레이트 후보」 의 예외가 필요하면 `deny.toml` 의 `[[licenses.exceptions]]` 와 `about.toml` 의 `[<크레이트>] accepted` 두 곳에 함께 넣는다.

## 위험 · 모르는 것

- **PG 에서 N행 뒤 끊기**: DBX 는 `row_limit` 에 닿으면 스트림을 끊는다(서버 커서 · 포털 없음). 서버가 나머지 행을 계속 보내는지, 연결을 버리는지는 확인하지 못했다. Pitwall 이 credit 스트림을 하려면 포털(`Execute` 의 max rows)이나 `DECLARE CURSOR` 가 필요할 수 있다 — 드라이버 선택(질문 1)과 엮인다.
- **sqlx 의 취소**: sqlx 0.9 에서 PG CancelRequest 를 보내는 공개 API 를 이번 검토에서 확인하지 못했다. DBX 의 취소 모델(실행 id → 토큰 → 드라이버 인터럽트)은 `tokio-postgres` 의 `CancelToken` 에 기댄다.
- **tokio-postgres 포크의 이유**: 루트 `Cargo.toml` 패치 줄에 설명 주석이 없다(GaussDB/openGauss 인증용으로 보이지만 적혀 있지 않다). 공개판으로 같은 동작이 되는지 모른다.
- **Redis 무한 대기**: DBX Redis 연결은 응답 타임아웃이 없고 취소도 없다. 느린 명령(`DEBUG SLEEP` 류는 차단되지만 큰 `LRANGE` 등)에서 무엇이 일어나는지 재지 않았다.
- **SQLite 뮤텍스**: `with_connection` 이 async 스레드에서 std 뮤텍스를 잡는 구조가 실제로 tokio 워커를 막는지 재현하지 않았다.
- **MCP 새 설치 기본값**: 「DDL 없는 쓰기 허용」 은 코드를 따라 읽은 결과다. 실행 확인 안 함.
- **JDBC 드라이버 라이선스**: Maven 으로 받는 드라이버와 커밋된 벤더 jar 의 라이선스는 확인하지 않았다.
- **GitHub macOS 러너의 Docker**: 쓸 수 없다고 알려져 있지만 이번에 재지 않았다.
- **aws-lc-sys 빌드 비용**: 라이선스는 통과지만 C 빌드 시간 · 툴체인 요구(cmake 등)는 재지 않았다.
- **규모**: DBX 핵심 파일들이 1만~1.8만 줄 단일 파일이고 기능이 서로 엮여 있다. 「크레이트째 차용」 은 현실적이지 않고, 차용 단위는 함수 · 모듈 수준이다.

## M3 계획 전에 사람이 정할 질문

1. **드라이버 크레이트**: 02 의 `sqlx`(PG · MySQL)를 유지하는가, DBX 처럼 네이티브(`tokio-postgres` + `deadpool-postgres`, `mysql_async`)로 가는가? 고려할 것 — DBX 근거가 사라졌다는 점, PG 취소(CancelRequest) · 포털 기반 행 스트리밍이 필요한지, 타입 정보를 얼마나 날것으로 받아야 하는지(DB 클라이언트는 임의 열을 다룬다).
2. **SQLite 드라이버**: `sqlx-sqlite` 는 지금 Pitwall 의 `rusqlite` 0.40 과 같이 링크되지 않는다. SQLite 는 `rusqlite` 로 하는가(DBX 와 같음), 아니면 `rusqlite` 를 `libsqlite3-sys` 0.37 이하를 쓰는 판본으로 내리는가?
3. **드라이버 추상과 크레이트 분할**: 공통 trait(02 안) vs enum 디스패치(DBX)? `pw-db-driver-sqlite` 를 따로 두는가(DBX 에는 SQLite 전용 크레이트가 없다)? `pw-db-sql` 을 하나로 둘지 DBX 처럼 나눌지?
4. **SQLite 격리 워커**: 02 의 「격리된 워커 프로세스」 는 DBX 근거가 아니었다(DBX 워커는 SSH 원격용). 로컬 SQLite 를 별도 프로세스로 열 것인가, 메인 프로세스 + `interrupt` · `busy_timeout` 으로 충분한가? 정하면 02 「DB 계층」 절을 고친다.
5. **위험도 분류**: DBX `sql_risk.rs` 의 AST 부분을 고지와 함께 차용해 고치는가, 직접 쓰고 DBX 시험 케이스만 참고하는가(케이스를 옮기면 그것도 고지 대상)? 그리고 —
   - F6-9 의 다섯 갈래를 그대로 등급으로 둘지(DBX 는 DROP·TRUNCATE 를 `Ddl` 에 합치고 WHERE 없음은 별도 플래그),
   - 파싱 실패를 fail-closed(쓰기 취급)로 할지,
   - 부작용 함수를 거부 목록(DBX)으로 할지 허용 목록으로 할지,
   - 여러 문장의 합산 순서(DBX 는 `Transaction` 이 `Ddl` 보다 위).
6. **read-only 강제 방식**: 데스크톱의 read-only 연결과 프로덕션 연결에 DB 수준 읽기 전용(PG `BEGIN READ ONLY` / `default_transaction_read_only`, MySQL `START TRANSACTION READ ONLY`, SQLite `query_only`)을 분류와 함께 거는가? MCP `db_query` 는 05 가 이미 「읽기 전용 트랜잭션」 으로 정했다.
7. **결과 전송 · 행 상한**: 콘솔 결과를 credit 스트림으로 끝까지 흘리는가, 상한(DBX 데스크톱 100,000 · 백엔드 10,000 · MCP 100~1000)에서 자르는가? 다음 페이지는 서버 커서로 이어 읽는가, LIMIT 를 덧씌워 다시 실행하는가(DBX)?
8. **값 표현 규칙**: bigint(±2^53 밖) · decimal · money 는 문자열, bytea 는 `0x` hex, json 은 문자열, timestamptz 는 서버 세션 시간대 텍스트 — 이 규칙을 RPC 타입으로 고정하는가?
9. **그리드 편집 범위(F6-8, P1)**: PK 없는 테이블의 편집을 허용하는가(PG `ctid` · `xmin`, 그 밖은 전 열 WHERE + 1행 가드)? 되돌리기 SQL 을 함께 만드는가? 수동 커밋 탭에서의 편집을 그 트랜잭션에 넣는가?
10. **TLS 기본값**: libpq `sslmode` 의미를 그대로 따르는가(`prefer` 는 평문 폴백, `require` 는 인증서 미검증)? 루트 인증서는 `webpki-roots`(CDLA 예외 필요)인가 OS 신뢰 저장소(`rustls-platform-verifier` 등)인가?
11. **SSH 터널 구현**: in-process `russh`(DBX — TOFU 검증기 차용 가능) vs 시스템 `ssh -L`(08 의 Relay 접속과 같은 원칙: `~/.ssh/config` · 에이전트 · ProxyJump · known_hosts 를 그대로 존중)? `russh` 면 `rsa` 기능을 끌 것인가(RSA 키는 에이전트로만), RUSTSEC-2023-0071 을 이유와 함께 `advisories.ignore` 에 넣을 것인가? 레거시 알고리즘(SHA-1 KEX · CBC) 폴백을 둘 것인가?
12. **자격 증명**: 비밀마다 키체인 항목(03 의 `secret_ref = "keychain"`)인가, DBX 처럼 키체인에는 암호 키 하나 + 암호문은 `$STATE` 의 SQLite 인가? 원격 Relay 에 비밀을 넘기는 규칙(08 「필요한 것만 세션 동안」)과 어떻게 맞추는가?
13. **미신뢰 프로젝트의 DB 설정**: 클론한 저장소의 `.pitwall/db/*.toml` 로 — 연결 · SSH 터널을 자동으로 여는가, 루트 밖 SQLite 파일을 여는가, SQLite 확장 로드를 허용하는가? git(13 결정 M2-D8)처럼 신뢰 전에는 무엇을 막는가?
14. **라이선스 · 공지 예외**: `webpki-roots`(CDLA-Permissive-2.0), `xxhash-rust`(BSL-1.0, redis-rs 를 쓰면 피할 수 없음), RUSTSEC-2023-0071 중 무엇을 `deny.toml` 의 크레이트 단위 예외로 받는가? 받지 않으면 대안은 각각 OS 신뢰 저장소 기능 · `fred` · 시스템 ssh 다.
15. **Redis**: 콘솔의 허용 목록 분류(모르는 명령 차단)를 차용하는가? 응답 타임아웃 · 취소를 둘 것인가(DBX 는 없다)? 클러스터 · Sentinel 은 F6-2 대로 P1 로 미루는가?
16. **시험 DB 방식**: 실 DB 시험을 어디서 돌리는가 — ubuntu 잡의 `services:` 컨테이너 / testcontainers, macOS 잡의 Homebrew 설치, `#[ignore]` live + 환경 변수 중 어느 조합인가? M3 완료 기준에 「세 DB 의 실 시험 N개 실행」 을 넣는가?
17. **문서 정정**: 02 「결론」 표 DB 행 · 「DB 계층: DBX 구조 참고」 절, 09 「참고 프로젝트」 의 DBX 행에 있는 사실 오류(DBX 가 sqlx 를 쓴다, SQLite 격리 워커)를 위 1 · 4 의 결정과 함께 고치는가?

## 다시 재는 법

받은 코드를 다시 받는다(빈 폴더에, 읽기만):

```bash
git -c core.hooksPath=/dev/null clone --depth 1 --no-recurse-submodules https://github.com/t8y2/dbx <빈 폴더>/dbx
git -C <빈 폴더>/dbx log -1 --format='%H %cI'   # 이 보고서는 c5e7e0430653aaef198800a48f9dec46a7cfed5e
```

크레이트 메타데이터:

```bash
curl -s -A '<연락처가 든 User-Agent>' https://crates.io/api/v1/crates/<크레이트>               # 최신 안정판 · 라이선스
curl -s -A '<연락처가 든 User-Agent>' https://crates.io/api/v1/crates/<크레이트>/<판>/dependencies
```

정책 프로브 — DBX 폴더 밖의 빈 폴더에 아래 세 매니페스트(각 `src/main.rs` 는 `fn main(){}`)를 만들고, 이 저장소의 `deny.toml` 을 각 매니페스트 옆에 복사한 뒤 저장소 루트에서 돌린다(빌드하지 않는다):

```bash
mise exec -- cargo generate-lockfile --manifest-path <프로브>/Cargo.toml
mise exec -- cargo-deny --manifest-path <프로브>/Cargo.toml check licenses bans sources
mise exec -- cargo-deny --manifest-path <프로브>/Cargo.toml check advisories
mise exec -- cargo tree --manifest-path <프로브>/Cargo.toml -e features -i <크레이트>   # 어느 기능이 끌어오는지
```

```toml
# 프로브 a — DBX 식 조합. licenses: webpki-roots · xxhash-rust 거절 / advisories: RUSTSEC-2023-0071
[dependencies]
tokio-postgres = { version = "0.7", features = ["with-chrono-0_4", "with-uuid-1", "with-serde_json-1"] }
deadpool-postgres = "0.14"
tokio-postgres-rustls = "0.14"
mysql_async = { version = "0.37", default-features = false, features = ["default-rustls", "chrono", "rust_decimal"] }
rusqlite = { version = "0.40", features = ["bundled"] }
redis = { version = "1.8", features = ["tokio-comp", "tokio-rustls-comp", "sentinel", "cluster-async"] }
russh = { version = "0.65", default-features = false, features = ["ring", "rsa", "flate2"] }
keyring = "4"
sqlparser = { version = "0.63", features = ["visitor"] }
[dev-dependencies]
testcontainers = "0.27"
testcontainers-modules = { version = "0.15", features = ["postgres", "mysql", "redis"] }
```

```toml
# 프로브 b — sqlx(PG · MySQL). licenses: webpki-roots 거절 / advisories: 통과
[dependencies]
sqlx = { version = "0.9", default-features = false, features = ["runtime-tokio", "tls-rustls-ring-webpki", "postgres", "mysql", "chrono", "uuid", "json", "rust_decimal"] }
```

```toml
# 프로브 c — sqlx-sqlite 와 Pitwall 의 rusqlite. generate-lockfile 이 links = "sqlite3" 충돌로 실패
[dependencies]
sqlx = { version = "0.9", default-features = false, features = ["runtime-tokio", "sqlite"] }
rusqlite = "0.40"
```

각 매니페스트에는 `[package]`(`name` · `version = "0.0.0"` · `edition = "2021"` · `license = "MIT"` · `publish = false`)를 붙인다.
