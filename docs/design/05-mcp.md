# 05. MCP 서버 설계

에이전트(Claude Code, Orca 내 에이전트 등)가 Pitwall을 통해 **프로젝트 구성 · 설정 · 조회**를 할 수 있게 한다.
코드 편집 자체는 에이전트가 파일시스템에서 직접 하므로, MCP는 "IDE만 아는 정보와 설정"에 집중한다.
같은 툴을 명령줄(`pitwall tool`)로도 부른다 — 「CLI (`pitwall tool`)」 절.

## 1. 전송 방식

| 방식 | 사용 | 비고 |
|---|---|---|
| stdio | `pitwall mcp [--project <path>]` | 실행 중 앱에 로컬 소켓으로 붙는 얇은 프록시. 앱이 꺼져 있으면 헤드리스 host 기동 |
| Streamable HTTP | `http://127.0.0.1:<port>/mcp` | M7 — Relay 의 MCP 엔드포인트와 함께 온다(06-roadmap.md 결정 D59). 앱 실행 중에만. Bearer 토큰 필수 |

- `--project` 미지정 시 cwd로 프로젝트 자동 해석 (F2와 같은 규칙)
- 앱 미실행 + 헤드리스 모드: 코어 툴만 있고 승인이 필요한 툴은 거부한다(「로컬 소켓과 헤드리스」). 승인을 건너뛰는 플래그는 두지 않는다
- MCP 서버는 Host 별 연결 풀을 두고 툴 호출의 `projectId` 로 라우팅한다([03-architecture.md](03-architecture.md) 「RPC 설계」) — 원격 Host 가 생기는 M7 부터다. 그때까지 세션은 로컬 Host 하나에 붙는다

### 로컬 소켓과 헤드리스

구현: `crates/pw-mcp`(`socket` · `proxy` · `server`), `apps/desktop/src-tauri/src/mcp.rs`.

- **앱 쪽**: 주 인스턴스가 시작하면 로컬 소켓을 연다. 세션 · 툴 · 감사 로그는 앱 안에서 돈다. 네트워크 포트는 열지 않는다
- **`pitwall mcp`**: 그 소켓이 있으면 붙어서 stdio 줄을 그대로 잇는 프록시다. 소켓이 없거나 받지 않으면 **헤드리스** — 같은 프로세스에 `LocalHost` 를 만들어 stdio 로 직접 서비스한다. 앱을 띄우지 않는다
  - 헤드리스는 레지스트리를 **읽기만** 한다: 파일이 없으면 빈 목록, 손상 · 마이그레이션이 필요한 파일도 건드리지 않고 빈 목록이다(복구 · 마이그레이션은 레지스트리를 소유한 앱의 일). 헤드리스에는 **코어 툴만** 있다(「툴 레지스트리」 의 「세션의 툴은 그 Host 가 처리하는 것만」) — `project_list` · `project_info` · `settings_get`. 승인 경로가 없으므로 그 가운데 `read` 툴만 성공한다. 그 밖의 툴(git · DB · 코드)은 앱이 실행 중이어야 한다 — 연 연결 · 승인 창 · 언어 서버 · 심볼 색인이 앱에 있다(06-roadmap.md 결정 D61)
    - 연결은 쓰기 가능한 플래그(파일은 만들지 않는다)로 열고 질의를 `query_only` 로 막는다. state.db 가 WAL 이라(03 「state.db」) `SQLITE_OPEN_READ_ONLY` 로 열면 앱이 꺼져 있을 때 `-wal` · `-shm` 을 만들고 닫으며 지우지 못해 남긴다. 쓸 수 있는 연결은 열려 있는 동안만 둘을 두고 마지막으로 닫힐 때 걷는다. 앱이 비정상 종료로 남긴 `-wal` 이 있으면 닫히며 그 내용(이미 커밋된 쓰기)을 파일에 옮긴다. 쓸 수 없는 폴더면 읽기 전용 · `immutable` 연결로 연다
  - Windows 는 소켓이 아직 없어 언제나 헤드리스다
  - **헤드리스로 시작한 뒤 앱이 뜨면** 그 뒤의 요청을 앱으로 넘긴다: 헤드리스 세션이 `tools/list` · `tools/call` 을 받을 때 소켓을 다시 보고, 앱이 받으면 넘겨 붙고 `tools/list_changed` 를 보낸다. 에이전트 클라이언트는 세션을 시작할 때 `pitwall mcp` 를 띄우므로 앱보다 에이전트를 먼저 켜는 순서가 흔하다. 아직 없다(16-m5-plan M5-20) — 지금 `pitwall mcp` 는 시작할 때 한 번만 앱을 찾는다. 넘겨 붙기가 들어오기 전까지는 헤드리스 세션의 서버 안내문(`instructions`)이 「앱이 꺼져 있어 코어 툴만 있다. 앱을 켠 뒤 MCP 서버를 다시 시작하면 나머지가 나온다」 를 알린다(`pw_mcp::server::HEADLESS_NOTICE`). 앱 세션에는 싣지 않는다. 헤드리스인지는 연결 방식(stdio · 소켓 · 프로세스 안 duplex)이 아니라 세션을 서비스하는 Host 의 성질이다 — 세션을 만드는 쪽이 `pw_mcp::Services` 의 `host_kind`(`HostKind::App` · `HostKind::Headless`, 기본값 없음)로 알린다. `write` 이상 거절의 까닭 `headless_write` 도 이것으로 가른다
- **위치**: `$STATE/run/<해시>.sock`. 경로가 `sun_path` 한도(103바이트, macOS 104 · Linux 108 의 작은 쪽)를 넘으면 `<임시 폴더>/pitwall-<uid>/<해시>.sock`(macOS `$TMPDIR` 는 사용자 전용). 양쪽이 같은 규칙으로 고른다
- **그 사용자만 붙는다**: 폴더는 0700 · 이 사용자 소유 · 심링크 아님(서버는 만들거나 0700 으로 고치고, 다른 사용자 소유 · 심링크면 소켓을 열지 않는다. 클라이언트는 같은 검사를 통과한 폴더에만 붙는다), 소켓 파일 0600, 연결마다 양쪽이 상대 uid(`getpeereid` · `SO_PEERCRED`)를 본다(`pw_mcp::socket::peer_allowed` — 다른 uid · 자격을 못 읽은 연결은 거절). macOS 는 소켓 파일 권한으로 connect 를 막지 않으므로 폴더 권한이 실제 방어선이다
- **워크트리 격리**: 소켓 이름의 해시는 단일 인스턴스 키(debug: 빌드 폴더 + `$DATA`, release: 앱 identifier — 기본이 아닌 `$DATA` 면 그 해시를 붙인 것, `apps/desktop/src-tauri/src/instance.rs`)와 `$STATE` 로 만든다. 다른 워크트리 · 다른 `$DATA` 의 `pitwall mcp` 는 다른 이름을 계산해 이 앱에 붙지 않고 헤드리스로 돈다
- **이미 있는 소켓**: 붙어 보고 받으면 다른 앱이 쓰는 중이라 열지 않는다. 받지 않으면(앱이 죽고 남은 파일) 지우고 연다. 소켓이 아닌 파일 · 심링크는 지우지 않는다. 앱이 끝날 때 소켓 파일이 남을 수 있는데, 다음 시작이 이 규칙으로 바꾼다
- **핸드셰이크**: 연결 직후 클라이언트가 JSON 한 줄(`protocol: "pitwall-mcp/1"`, `project`, `cwd`), 서버가 한 줄(`ok`, `error`)로 답한다. 그 뒤는 MCP 바이트다. 프로젝트 해석(`project.resolve`)은 앱이 한다
- **세션 등록부**(16-m5-plan M5-10, `pw_mcp::sessions`): 앱 안의 세션은 초기화를 마치면 등록되고 끝나면 빠진다 — id(감사 행의 `session`) · 클라이언트(`이름 버전` — 2025-11-25 까지의 클라이언트는 initialize 로, 2026-07-28 클라이언트는 첫 툴 호출로 안다) · 세션 프로젝트 · 전송 · 시작 시각 · 호출 수(감사 행 하나가 호출 하나). 사람의 화면은 `mcp.sessions`(read, owner — 에이전트 범위 연결은 거절한다)로 읽고, 열고 닫힐 때마다 Host 범위 이벤트 `mcp.sessionsChanged`(payload 는 열린 수 `total` 뿐)가 한 번씩 나간다. 앱 없이 뜬 헤드리스 세션은 그 프로세스의 등록부라 앱의 목록에 없다. 화면: 프로젝트 창 상태바의 「MCP n」(그 프로젝트를 세션 프로젝트로 가진 세션 수, 0 이면 없음 — 누르면 세션 목록 팝업, 「에이전트 활동」 패널(M5-13) 전까지) · 웰컴 창 아래 줄의 「MCP: 연결 n」(전부)
- **끊김**: 앱이 도중에 닫히면 프록시는 응답을 기다리던 요청마다 JSON-RPC 오류(`-32001`)를 stdout 에 쓰고 stderr 에 이유를 남긴 뒤 **종료 코드 1** 로 끝난다. 클라이언트가 stdin 을 닫으면 소켓의 쓰기 쪽을 닫아 앱 쪽 세션을 끝내고(남은 응답은 5초까지 옮긴다) 0 으로 끝난다. 앱 쪽 세션이 끝나면 그 세션의 Host 연결 · 설정 감시 · 버스 구독도 걷힌다
- **호출 취소**: 클라이언트가 호출을 취소하거나(`notifications/cancelled` — rmcp 가 그 요청의 취소 토큰을 건다) 세션이 끊기면 그 호출의 Host RPC 를 취소한다 — `pw_mcp::HostClient` 가 그 요청 id 로 `request.cancel` 을 보내고 Host 는 핸들러 future 를 버린다. 응답 전에 호출 future 가 버려져도 같다(`HostClient::call_until` · `call`). 세션이 끊긴 경우는 rmcp 가 남은 응답을 5초까지 기다린 뒤 취소 토큰을 건다. 취소된 호출은 감사 로그에 결과 `error` · 상세 `cancelled` 로 남고 결과 표지의 까닭도 `cancelled` 다. CLI 의 Ctrl+C · `db_query` 의 취소 · 승인 대기 거두기는 이 길만 쓴다
- **세션은 프로젝트 설정을 쓰지 않는다**: 노출 판정용으로 세션 프로젝트의 설정을 읽을 때 구버전 파일은 메모리에서만 올린다(`ConfigSource::without_writes`). 커밋된 `.pitwall/project.toml` 에 `schema` 가 써지거나 저장소에 `.backup/` 이 생기지 않는다. 그 설정의 이벤트는 세션 전용 버스로 내 앱의 버스에 같은 프로젝트의 `config.*` 가 한 번 더 나가지 않는다
- **stdout 에는 MCP 바이트만**: 로그는 `$STATE/logs/mcp.<날짜>.log`, 경고 이상은 stderr 에도 쓴다. `pitwall mcp` 는 확장을 읽지 않는다
- 사용법 오류는 종료 코드 2

## 2. 툴 목록

### 프로젝트
| 툴 | 설명 | 권한 |
|---|---|---|
| `project_list` | 등록된 프로젝트 목록 | read |
| `project_info` | 루트, 언어, 감지된 빌드 툴, 설정 요약 | read |
| `project_register` | 폴더를 프로젝트로 등록 (+ 자동 감지 결과 반환) | write |
| `project_open` | UI에서 프로젝트/파일/라인 열기 | read* |
| `settings_get` | 설정 키 조회(전역 · 프로젝트 — 디스크에 쓰지 않고 읽는다) | read |
| `settings_set` | 설정 키 변경 — M5 에 없다. 설정 UI(04 F11-6)가 설정 키 표를 만들 때 함께 온다(06-roadmap.md 결정 D59) | write |

**지금 구현**(16-m5-plan M5-09):

- `settings_get`(← 툴 메서드 `config.tool.get`, `crates/pw-core/src/config/tool.rs`, 코어 툴 — 헤드리스에도 있다): `{ projectId?, worktree?, key? }`. 부를 때마다 파일을 디스크에 쓰지 않고 새로 읽는다(`ConfigSource::without_writes` — 구버전 파일을 마이그레이션해 쓰거나 `.backup/` 을 만들지 않는다). `projectId` 가 없으면(`null`) 전역 설정, 있으면 전역 위에 그 프로젝트 설정(`project.toml` · `project.local.toml`)을 얹은 값이다 — 세션에 프로젝트가 있으면 키를 빼면 세션 프로젝트이므로 전역만 보려면 `null` 을 준다. 결과는 값 하나(테이블이 아닌 것)마다 점 키(`mcp.preset`, 점 · 공백이 든 키 이름은 큰따옴표) · 값 · 이긴 레이어(`builtin` · `global` · `shared` · `personal` — 개인 전용 프로젝트의 `project.toml` 은 `personal`) · 가렸는가와, 읽은 파일 목록이다. `key` 는 점 키 단위 접두로 좁힌다. 키 이름이 비밀을 가리키는 값은 `[redacted]` 로 가린다(`pw_core::secret` — 감사 로그 인자 요약과 같은 한 벌, 배열 안 테이블의 키까지). 그 밖의 값(실행 파일을 정하는 `[git] path` · `[open_with.tools]` · `[lsp.<id>]` 의 명령 포함)은 파일 그대로다 — 에이전트가 파일을 직접 읽는 것과 같은 범위(메인 판정 2026-11-14). 파일을 읽지 못하면 그 파일 경로만 알리고 파서의 오류 글(문제 줄의 값을 인용한다)은 싣지 않는다
- `project_open`(← 툴 메서드 `project.tool.open`, `crates/pw-core/src/open.rs`, 앱 Host 만 — Host 를 만들 때 「열기 요청」 자리 `pw_core::open::OpenRequests` 를 받은 Host 에만 붙는다): `{ projectId, worktree?, path?, line?, column? }`. Host 가 관문(`agent_grant`) · 경로(프로젝트 루트 안 — 상대 경로는 루트 기준, 판정은 `fs.*` 와 같은 정규화로 심링크를 푼 실제 경로, 밖이면 invalid params) · 빈도를 보고, 데스크톱(`apps/desktop/src-tauri/src/agent_open.rs`)이 `pitwall <path>` 와 같은 길(창별 보류 큐 · `project://open`)로 연다 — 다른 점은 창을 앞으로 가져오지 않는 것이다. 새 창은 Tauri 의 `focused(false)` 로 만든다(macOS 는 tao 가 `makeKeyAndOrderFront` 대신 `orderFront` 만 부르고 앱을 활성화하지 않는다 — 키보드 포커스는 사람이 쓰던 앱에 남고, 창은 그 위에 보일 수 있다). 결과는 `openedIn`(`existingWindow` · `newWindow`)과 연 파일의 실제 경로. **빈도**: Host 전체에 한 벌 — 그 Host 의 에이전트 호출(MCP 세션 · `pitwall tool` 전부)을 합쳐 10초에 5번(`pw_core::open::RATE_LIMIT` · `RATE_WINDOW` — 출발값), 넘으면 `PERMISSION_DENIED`(`data.reason = "rateLimited"`, `retryAfterMs`) — 결과 표지 `rate_limited`. 메인 판정 2026-11-14: Host 전체 한도, `pitwall tool` 이 호출마다 새 세션이라서(세션마다 세면 셸 반복 한 줄로 풀린다) — 세션별 한도는 따로 두지 않는다. 세는 것은 여는 데까지 간 호출이다(입력이 틀렸거나 앱이 아직 창을 열 수 없어 거절한 호출은 세지 않는다). 사람의 열기와 같이 최근 연 순서(레지스트리의 `opened_at`)를 갱신한다. 헤드리스에는 없어 CLI 는 `needs_app`(종료 3)으로 가른다

### 실행 구성 (M6 — 실행 기능과 함께 제공)
| 툴 | 설명 | 권한 |
|---|---|---|
| `run_config_list` | 실행 구성 + 감지된 태스크 | read |
| `run_config_upsert` | 실행 구성 생성/수정 (스키마 검증) | write |
| `run_config_delete` | 삭제 | write |
| `run_start` / `run_stop` / `run_restart` | 구성 실행 제어 | exec |
| `run_status` | 실행 중 프로세스, 포트, 종료 코드 | read |
| `run_logs` | 로그 조회 (tail, since, grep 필터) | read |

**`sql` 종류 실행 구성**(04 F7-1, 06-roadmap.md 결정 D55): DB 연결과 SQL 파일 목록을 저장한 구성도 위 툴로 만들고 돌리고 로그를 본다. 프로세스를 띄우지 않고 `db_query` · `db_execute` 와 같은 DB 실행 경로를 지난다.

- **등급은 `exec` 규칙이 아니라 실행하는 그때의 분류가 정한다.** `run_start` 가 부를 때마다 파일들을 읽어 분류하고, 조회만이면 `read`, 그 밖은 `dangerous`(매번 승인 · 프로덕션 기본 차단)다. 「이미 존재하는 구성은 자동」 은 `sql` 종류에 적용하지 않는다 — 구성이 그대로여도 파일 내용은 바뀔 수 있다
- 연결의 read-only · 프로덕션 태그 · 미신뢰 프로젝트의 연결 금지는 SQL 콘솔과 같이 적용된다
- `run_logs` 에는 문장마다 번호 · 영향 행 수(조회는 행 수와 앞쪽 몇 행) · 소요 · 오류가 남는다. 결과 행 전체가 필요하면 `db_query` 를 쓴다

### 디버그
| 툴 | 설명 | 권한 |
|---|---|---|
| `breakpoint_list` / `breakpoint_set` | 브레이크포인트 조회/설정 | read / write |
| `debug_state` | 현재 정지 위치, 스택, 지역 변수 스냅샷 | read |

### DB
| 툴 | 설명 | 권한 |
|---|---|---|
| `db_connection_list` | 연결 목록 (비밀 제외) — 대상(호스트 · 포트 · DB · SQLite 경로), 에이전트에게 적용하는 read-only · 프로덕션 표지, 상태, 에이전트가 지금 쓸 수 있는가(`usable` · `unusableReason`). 미신뢰 프로젝트에서도 준다 | read |
| `db_connection_upsert` | 연결 추가/수정 — 비밀번호는 툴 인자로 받지 않고 UI 입력 요청 또는 env 참조 | write |
| `db_connection_test` | 연결 테스트 — 입력은 연결 id 뿐이다(저장 전 초안 · 비밀은 받지 않는다). 연 적 없는 연결이면 「이 연결 열기」 승인을 청한다 | write |
| `db_schema` | 스키마/테이블/컬럼/인덱스 조회 — `object` 가 없으면 한 단계의 목록 한 쪽(`kind`: `database` · `schema` · `table`(기본) · `view` 등, `filter` · `cursor` · `limit`), 있으면 그 객체의 열(기본 키 표시) · 인덱스 · FK, `detail: "ddl"` 이면 DDL. 창의 탐색기와 같은 메타데이터용 연결 · 캐시를 쓴다 | read |
| `db_query` | 조회 SQL 실행 — 입력은 `sql`(글) 또는 `file`(SQL 파일 경로) 중 하나와 `maxRows`(문장마다 행 상한 — 쪽 단위가 아니다, 메인 판정 2026-11-14). **읽기 전용 트랜잭션 + 행 제한**. 조회가 아닌 문장이 하나라도 있으면 거절한다 | read |
| `db_execute` | 쓰기를 포함한 SQL 실행 — 입력은 `db_query` 와 같다. SQL 위험도 분류(04 F6-9)의 문장과 등급을 보이고 **매번 승인**받는다 | dangerous |
| `db_kv_scan` | Redis 키를 쪽 단위로 찾는다(패턴 — 키 · 타입 · TTL · 크기) | read |
| `db_kv_get` | Redis 값 한 쪽(타입별) | read |

**툴의 등급은 툴 메서드의 선언이다.** UI 가 쓰는 RPC(`db.schema.*` · `db.query.page` · `db.connection.test`)는 연결을 열어 `exec` 로 선언돼 있고 툴이 아니다(「툴 레지스트리」 의 「툴 메서드」). DB 쓰기는 조회와 다른 툴이다 — `db_execute` 만 따로 끄고 켤 수 있다(06-roadmap.md 결정 D60). `db_schema` · `db_query` · `db_execute` 는 SQL 드라이버(PG · MySQL/MariaDB · SQLite)만 받고 Redis 연결이면 `unsupported` 다. Redis 는 조회 둘(`db_kv_scan` · `db_kv_get`)뿐이고 명령 실행 툴은 없다.

**에이전트의 DB 연결**(06-roadmap.md 결정 D60 — D48 「연결은 사람이 누를 때만」 의 에이전트 쪽):

- 연결을 여는 툴(`db_schema` · `db_query` · `db_execute` · Redis 조회 · `db_connection_test`)은 **사람이 그 프로젝트 창에서 이번 앱 실행 중에 같은 대상으로 연 연결**만 쓴다. 「같은 대상」 은 푼 대상(드라이버 · 호스트 · 포트 · DB · 사용자 · 터널 목적지)이 같다는 뜻이다 — 연결 설정은 워크트리마다 다른 파일에서 읽히므로, 워크트리의 같은 id 가 다른 대상을 가리키면 연 적 없는 연결이다
- 툴은 사람의 연결을 빌리지 않는다 — 호출마다 자기 연결을 새로 연다(스키마는 메타데이터용 연결). 사람이 DB 탭을 닫은 뒤에도 앱을 끌 때까지 된다
- 연 적 없는 연결은 `db_schema` · `db_query` · `db_execute` · Redis 조회가 거절한다. **`db_connection_test` 만 「이 연결 열기」 승인을 청한다** — 그 세션 동안 그 연결(같은 대상)에 한하고 `auto_approve` 를 적용하지 않는다. 승인받은 연결은 그 세션의 다른 DB 툴도 쓴다
- 프로덕션 태그 연결은 `[mcp.policy] production` 을 따른다 — `block`(기본)이면 조회도 쓰기도 막고, `ask` 면 **사람이 연** 프로덕션 연결에 한해 조회까지 매번 승인받는다. 연 적 없는 프로덕션 연결은 승인으로도 열리지 않는다. `ask` 는 사람의 설정에서만 켠다 — 커밋되는 공유 설정의 `[mcp.policy]` 는 읽지 않는다(「권한 모델」)
- **표지는 조여지기만 하고 대상에만 묶인다**(16-m5-plan 결정 M5-D37): Host 가 그 프로젝트의 연결 설정을 읽을 때마다(목록 · 열기 · 툴 호출 — 어느 워크트리의 것이든) 그 연결의 프로덕션 · read-only 표지를 푼 대상에 묶어 기억에 더한다 — 연결 id 와 무관하고 사람이 그 연결을 열었는지와도 무관하다(「사람이 연 연결」 은 그대로 연결 id 와 대상 둘에 묶인다). 에이전트의 DB 툴 호출 때는 그 저장소의 메인 체크아웃(연결 워크트리를 따로 등록한 프로젝트면 신뢰가 따르는 메인 저장소 프로젝트의 주 루트 — 메인 판정 2026-11-14)의 연결 설정도 함께 읽어 더한다. **표지의 열쇠**(메인 판정 2026-11-14 — M5-06 리뷰): 한 연결로 다른 DB 를 읽을 수 있는 드라이버(MySQL · MariaDB)는 표지의 열쇠에서 DB 이름을 뺀다 — 열쇠는 드라이버 · 호스트 · 포트 · 사용자 · 터널 목적지다. 그 서버의 어느 DB 가 프로덕션이면 그 서버 · 사용자로 가는 에이전트 호출은 모두 프로덕션이다(`db_query` 가 SQL 글 안의 다른 DB 를 지킬 수 없다). Redis 도 DB 번호를 뺀다(메인 판정 2026-11-17 — 16-m5-plan M5-19): 한 연결이 `SELECT` 로 다른 번호로 옮겨 가고 번호 사이에 권한 경계가 없으며, 번호를 열쇠에 두면 같은 번호의 다른 표기(`database` 입력 `"00"`, 번호를 뺀 설정과 `"0"`)로 표지를 비껴간다 — 그 서버의 어느 번호가 프로덕션이면 그 서버 · 사용자의 모든 번호가 프로덕션이다. PG 는 DB 이름을 열쇠에 둔다(한 연결이 다른 DB 를 읽지 못한다). SQLite 는 파일이 열쇠다(`ATTACH` 는 M5-07 의 실행 분류가 다룬다). 「사람이 연 연결」 의 열쇠는 그대로 푼 대상 전체다. 에이전트에게 적용하는 표지는 이번 앱 실행 중 그 대상에서 본 것 가운데 가장 엄한 것이다. 설정에서 `production` 이나 `read_only` 를 지워도, 자기 워크트리에서 표지 있는 연결을 지우고 표지 없는 새 연결 설정(다른 id)으로 같은 대상을 가리켜도, 메인 체크아웃의 설정이 그 대상을 프로덕션이라고 적고 있으면 에이전트에게는 프로덕션이다 — 조회 길과 「이 연결 열기」 승인 길 둘 다, 사람이 그 연결을 연 적이 없어도. 비밀과 확인이 대상에 묶이는 것과 같다(같은 대상은 같은 데이터다). 표지를 더하는 쪽은 곧바로 따른다. 그 프로젝트의 어느 연결 설정도 표지를 단 적 없는 대상은 기억한 표지가 없어 지금 설정의 표지를 따른다(같은 서버의 다른 사용자 · PG 의 다른 DB 이름은 다른 대상이다) — 「이 연결 열기」 승인 다이얼로그가 대상(호스트 · DB)을 보인다는 것이 그 자리의 방어다
- 미신뢰 프로젝트 · read-only 연결 · 비밀과 확인의 규칙은 창과 같다(04 F6-9). 비밀이나 확인이 필요하면 툴 오류로 끝난다 — 에이전트는 답할 수 없다
- **지금 구현**(16-m5-plan M5-06): 툴 메서드는 `agent_grant` 를 지난 뒤 `pw_db::gate::db_grant_for_agent` 를 지난다 — 신뢰 게이트 → 연결 설정 읽기(그 저장소의 메인 체크아웃과 그 워크트리 — 둘 다 표지를 기억에 더한다) → 프로덕션 표지 → 「사람이 연 연결」. 거절은 `PERMISSION_DENIED` 이고 `data.reason` 이 `untrusted` · `productionBlocked` · `notOpenedByUser` 다(결과 표지 `denied` — 「툴 레지스트리」, CLI 종료 3). 「사람이 연 연결」 은 그 프로젝트 창의 증거로 저장한 연결을 드라이버까지 연 (프로젝트, 연결 id, 푼 대상)이다 — 세션 · 탐색기 · 쪽 읽기 · 연결 테스트가 모두 그 자리를 지나고, 저장 전 초안의 테스트는 세지 않는다. 메모리에만 있다. 「푼 대상」 의 호스트는 소문자로 맞추고 포트는 적지 않았으면 드라이버 기본이며, SQLite 면 파일의 실제 경로다(워크트리마다 상대 경로가 다른 파일이다). TLS 설정은 대상에 넣지 않는다 — `sslmode` 만 바꿔 표지를 비껴가지 못하게. 남는 한계: 호스트 별칭(`localhost` · `127.0.0.1` · IP), 터널 목적지의 다른 표기(`bastion` · `ssh://bastion.example.com`), DB 이름의 대소문자(대소문자를 가리지 않는 MySQL 서버 — macOS · Windows 기본 — 에서도)는 다른 대상으로 본다 — 「이 연결 열기」 승인 다이얼로그(M5-17)가 대상을 보이는 문구를 정할 때 이 셋을 쓴다. 외부 표(postgres_fdw · file_fdw, MySQL `FEDERATED`, MariaDB `CONNECT` · `FEDERATEDX` · Spider)는 원격 서버의 프로덕션 · read-only 표지를 보지 않는다. dev 연결의 DB 에 프로덕션을 가리키는 외부 표가 있으면 에이전트의 `db_query` 가 그 행을 읽는다(16-m5-plan M5-07 재리뷰 G1 — 막지 않고 한계로 둔다, 결정 M5-D38). 표지의 기억은 대상만 열쇠다(프로젝트를 가리지 않는다 — MySQL · MariaDB 는 DB 이름을, Redis 는 DB 번호를 뺀 서버 · 사용자가 열쇠다). `db_schema` 는 그 호출이 실제로 읽을 DB 마다(`database` 입력 · `object` 의 점 이름에서 나온 DB) 관문을 지난다. 틀린 연결 설정의 까닭 글(serde 오류 — 값을 싣는다)은 툴 응답 · 감사 로그로 내보내지 않는다. 받은 증거는 그 연결 id · 워크트리 · 대상에 묶여, 연결을 여는 자리(드라이버 연결 직전 · 스키마 캐시 앞)가 그때 다시 읽은 설정의 대상이 다르면 열지 않고 `notOpenedByUser` 로 거절한다. 툴이 연 메타데이터용 연결은 쉬는 시간(`pw_db::schema::IDLE_CLOSE`)으로만 닫힌다 — 세션이 닫혀도 둔다. 지금 연결을 여는 툴은 `db_schema` · `db_query` · `db_kv_scan` · `db_kv_get` 이다. 「이 연결 열기」 승인(M5-17)과 `production = "ask"`(M5-18)는 아직 없다 — 연 적 없는 연결 · 프로덕션 연결은 거절만 한다

**`db_query` · `db_execute` 의 `file`**(04 F6-14, 06-roadmap.md 결정 D55) — 한 번 돌리고 결과를 행으로 받는 길이다. 되풀이하는 스크립트는 「실행 구성」 표의 `sql` 종류로 저장한다.

- `sql` 과 `file` 은 함께 줄 수 없다. `file` 은 프로젝트 루트 안의 파일만 받는다 — 상대 경로는 프로젝트 루트 기준이고(MCP · CLI 같다, 부른 프로세스의 cwd 가 아니다. 워크트리가 정해진 호출이면 그 워크트리 안의 프로젝트 루트다), 판정은 `fs.read` 와 같아 심링크로 루트 밖을 가리키면 거절한다. UTF-8 텍스트, 편집 상한(06 결정 D33 의 5 MiB)까지
- **Host 가 실행하는 그때 파일을 읽고, 읽은 그 내용을 분류해 그 내용을 실행한다.** 판정은 `sql` 로 같은 글을 보냈을 때와 같다 — `db_query` 는 조회가 아닌 문장이 하나라도 있으면 아무것도 돌리지 않고 거절하고, `db_execute` 의 등급은 파일 전체에서 가장 높은 문장의 등급이다. 분류기는 Host 의 것 하나다
- **파싱 실패 · 너무 큰 문장이 든 호출은 `db_execute` 도 승인을 묻지 않고 거절한다**(파괴 등급으로 닫는 06 결정 D45 — 무엇인지 모르는 문장을 승인 다이얼로그에 올리지 않는다). 사람은 SQL 콘솔에서 돌린다
- `db_execute` 의 승인 요청은 파일 경로와 조회가 아닌 문장들을 보이고, 승인은 읽은 내용의 해시에 묶인다 — 승인 뒤 파일이 바뀌면 그 승인으로 돌지 않는다. 해시는 연결의 대상 · 프로덕션 · read-only 표지도 덮는다(「권한 모델」 의 「승인은 계획의 해시에 묶는다」)
- 문장마다 결과 하나(열 · 행 또는 영향 행 수 · 소요)를 차례로 돌려주고 행 제한은 문장마다 건다. `db_query` 는 읽기 전용 트랜잭션 하나로 돌고, 문장 하나가 실패하면 거기서 멈추고 그때까지의 결과와 오류(문장 번호 · 위치)를 돌려준다. `db_execute` 는 호출 하나를 트랜잭션 하나로 묶고 문장 하나가 실패하면 롤백한다 — 암묵 커밋하는 문장이 있으면 승인 요청에 「되돌릴 수 없다」 를 보이고, 트랜잭션 안에서 못 도는 문장은 DB 의 오류로 끝난다(그것을 푸는 「트랜잭션 방식」 은 실행 구성 `sql` 종류의 것이다 — M6)
- **`db_query` 의 지금 구현**(16-m5-plan M5-07): 입력 `{ projectId, worktree?, connectionId, sql | file, maxRows? }`. `maxRows` 는 문장마다 돌려줄 행의 상한(기본 100, 1 ~ 10000)이고 쪽 단위가 아니다 — 잘린 문장은 `truncated` 이고 다음 쪽이 없다. 다시 부르면 다른 스냅숏이라 에이전트는 WHERE · LIMIT 로 좁혀 다시 부른다(메인 판정 2026-11-14 — 툴 메서드 규약의 `limit` 은 쪽 단위 툴의 것이라 이름을 가른다. `db_execute` 도 같은 이름을 쓴다). 관문은 M5-06 의 입구(`db_grant_for_agent`) 하나 → 연결의 드라이버 방언 → `file` 읽기 → 분류 → 조회가 아닌 문장이 있으면 연결을 열지 않고 `PERMISSION_DENIED`(`data.reason = "notAQuery"`, `data.statements` 는 그 문장들의 번호 · 범위 · 등급 · 까닭 — 결과 표지 `denied` · `permission_denied`, 오류 글에 첫 문장의 번호 · 줄 · 등급과 「쓰기는 `db_execute`」). 조회 판정은 `db.query.page` 와 같은 `is_strict_query` 다 — 부작용 함수(`nextval` · `pg_sleep` · pg_surgery `heap_force_kill` …) · `SET` · 트랜잭션 제어 · 파싱 실패 · `tooLarge` · SQLite `ATTACH` · `PRAGMA` 는 조회가 아니다. **한계**(리뷰 B1): 이 판정은 알려진 함수 목록이라(`pw_db_sql::risk` 의 `SERVER_ADMIN_FUNCTIONS` · `SIDE_EFFECT_FUNCTIONS` · `SQL_TEXT_FUNCTIONS` — 훑은 범위와 방법은 그 머리 주석: PG 17.6 의 contrib 전부 · MySQL 8.4 · MariaDB 11.8 · SQLite 의 내장 함수), 사람이 설치한 contrib 밖 확장의 C 함수가 읽기 전용 트랜잭션을 우회하면 막지 못한다. postgres_fdw 의 외부 표 읽기도 막지 못한다 — 원격 서버에서는 읽기 전용이 아닌 트랜잭션으로 돌아(PG 17 기준), 원격의 뷰 · 함수가 쓰면 쓰기가 남는다(분류기는 표가 외부 표인지 모른다. file_fdw 의 `program` 외부 표도 같다). 실행은 새 연결의 읽기 전용 트랜잭션 하나에서 차례로, 문장마다 결과 `{ index, line, grade, columns, rows, rowsAffected, truncated, elapsedMs }`, 실패하면 거기서 멈추고 `error { index, line, range, error }`. 셀은 4 KiB 에서 잘린다(`truncated` 값). 응답 예산 192 KiB(`pw_db::script::RESPONSE_BUDGET`)는 **행을 읽는 그때** 센다(리뷰 B2): 행의 JSON 크기를 더하다 넘는 순간 그 문장은 `truncated` 이고 커서를 버리며, 뒤 문장은 돌리지 않고 결과의 `notRun { index, line, count, reason: "responseBudget" }`(돌리지 않은 첫 문장과 그 뒤 문장 수)으로 알린다. 한 번에 읽는 몫도 남은 예산으로 줄여 Host 가 쥐는 행이 예산 몫(+ 한 묶음)에 머문다. 결과의 `audit` 은 감사 상세(「감사 로그」)다. 호출이 취소되면 그 연결에 드라이버 인터럽트를 보내고 롤백한 뒤 닫는다. 실행 파이프라인은 `pw_db::script` 이고 `db_execute`(M5-18)가 쓰기 갈래를 더한다
- **`db_kv_scan` · `db_kv_get` 의 지금 구현**(16-m5-plan M5-19 — 결정 M5-D7): 툴 메서드 `db.tool.kvScan` · `db.tool.kvGet`(`crates/pw-db/src/kv_tool.rs`), 둘 다 `read` · 그룹 `db` · 앱 Host 만. 입력은 `{ projectId, worktree?, connectionId, database?, match?, cursor?, limit? }` · `{ projectId, worktree?, connectionId, database?, key, cursor?, limit? }` 이고 **명령 칸이 없다**. 창의 세션 id 를 받지 않고 호출마다 자기 연결을 연다(M5-06 의 관문 `db_grant_for_agent` 를 지나고, `database` 입력도 관문에 넘긴다 — DB 번호마다 연결 하나). `db_kv_scan` 은 키 한 쪽(키 · 타입 · TTL · 크기)과 `nextCursor` 다 — 스트림 `db.kv.scan` 이 툴이 될 수 없어 `SCAN` 커서를 한 쪽씩 도는 unary 다. `limit`(1 ~ 500, 기본 100)은 쪽의 상한이고 `SCAN` 한 걸음이 혼자 더 준 때만 넘는다. 쪽이 비어도 `nextCursor` 가 있을 수 있다. **`SCAN` 의 중복은 한 쪽 안에서 거르고 쪽 사이에서는 그대로 준다**(툴 설명에 적는다 — 끝까지 있던 키는 빠지지 않는다). `db_kv_get` 은 값 한 쪽(타입별 — string 은 64 KiB 바이트 창, hash · set 은 스캔 쪽, list · zset 은 번호, stream 은 항목 id, RedisJSON 은 통째)과 타입 · TTL · `total` · `nextCursor` 이고, 없는 키는 `missing` 이다. 응답 예산은 `db_query` 와 같은 192 KiB 다 — 스캔은 넘칠 묶음을 다음 쪽으로 넘기고, 값은 쪽의 원소 수(string 은 창)를 줄이고 원소 하나가 넘치면 그 값만 앞부분으로 자른다(`truncated`). 보내는 명령은 드라이버의 스캔 · 키 정보 · 값 읽기뿐이고(`SCAN` · `TYPE` · `PTTL` · `MEMORY USAGE` · 타입별 길이 · `GETRANGE` · `HSCAN` · `SSCAN` · `LRANGE` · `ZRANGE` · `XRANGE` · `JSON.GET`), 모두 분류기(`pw_db_sql::kv_command`)가 확인 없는 조회로 매긴다 — 연결을 열 때의 설정 명령(`AUTH` · `SELECT` · `CLIENT SETINFO` · `INFO server`)은 셈에 넣지 않는다(메인 판정 2026-11-17). 키 이름 · `match` 는 RESP 의 인자로만 간다(명령 글로 끼워 넣을 수 없다). 응답 타임아웃은 `[db] redis_response_timeout`. `db_schema` · `db_query` 는 Redis 연결에 `unsupported` 와 이 두 툴의 이름을 낸다
- `db_execute` 는 아직 없다(16-m5-plan M5-18)

### Git
| 툴 | 설명 | 권한 |
|---|---|---|
| `git_status` / `git_log` / `git_blame` / `git_diff` | 조회 | read |
| `git_branch_list` | 브랜치 | read |
| (쓰기 작업은 의도적으로 제외) | 에이전트는 git CLI를 직접 쓰면 되므로 중복 제공하지 않음. 예외는 git CLI 로 할 길이 없는 `checkout_send` 하나다(「체크아웃」) | |

`git_log` 은 스트림 `git.log` 가 아니라 쪽 단위 `git.logPage`(13-m2-plan M2-03 · M2-D7)에서 만든다.

**지금 구현**(16-m5-plan M5-05): `crates/pw-git/src/tools` 의 툴 메서드 다섯(`git.tool.status` · `git.tool.log` · `git.tool.diff` · `git.tool.blame` · `git.tool.branchList`). 모두 `read` · 그룹 `git` 이고, 입력은 `{ projectId, worktree?, … }` 이며 UI 의 git 읽기 RPC 와 같은 함수를 부른다. 앱의 Host 만 처리한다 — 헤드리스 `tools/list` 에는 없다(결정 M5-D3).

- **관문**: 툴마다 먼저 `agent_grant`(MCP 세션의 연결 범위 · 레지스트리에 있는 프로젝트)를 지난다. UI 의 git 읽기 RPC(`git.status` · `git.refs` · `git.diff` · `git.logPage` · `git.blame`)는 호출자를 보지 않지만, 툴 메서드는 창 · Host 범위의 호출자를 `PERMISSION_DENIED`(`notAgentSession`)로 거절한다. 없는 프로젝트는 그 관문의 invalid params 그대로다
- **미신뢰 프로젝트**: git CLI · 저장소 설정의 프로그램(필터 드라이버 · fsmonitor · textconv)을 띄우지 않는 규칙(03-architecture.md 「보안」)은 밑의 함수가 지킨다 — 툴이 따로 판정하지 않는다. `git_blame` 의 엔진도 `git.blame` 과 같다(신뢰한 프로젝트는 `git blame --incremental`, 미신뢰 · git 을 찾지 못하면 gix)
- **응답 크기**: 툴은 결과 JSON 192 KiB(`pw_git::tools::RESPONSE_BUDGET`, 세션 상한 256 KiB 아래) 안에서 자른다

| 툴 | 밑의 함수 | 입력(`projectId` · `worktree?` 말고) | 결과 |
|---|---|---|---|
| `git_status` | `git.status` · `git.refs` · `git.repo` | `paths?` · `cursor?` · `limit?`(1~1000, 기본 200) | 요약(`head` · `upstream`(ahead/behind) · 진행 중 작업 · 스테이징 · 미스테이징 · untracked · 충돌 파일 수 — 쪽이 아니라 전체의 수)과 바뀐 파일의 한 쪽(경로순), `nextCursor`(그 쪽의 마지막 경로) |
| `git_log` | `git.logPage` | `branch?`(없으면 HEAD — `git log` 와 같다) · `author?` · `message?`(둘 다 대소문자를 가리지 않는 글자 그대로) · `since?` · `until?`(git `--since` 꼴 — `2026-01-31` · `2 weeks ago` · `@<유닉스 초>`) · `paths?` · `detail?` · `cursor?` · `limit?`(1~500, 기본 50) | 커밋마다 해시 · 부모 · 작성자 · 작성 시각 · 제목 · 가리키는 ref 이름, `detail` 이면 본문(16 K 글자에서 자름). 그래프 레인은 싣지 않는다. HEAD 에 커밋이 없으면 빈 쪽 |
| `git_diff` | `git.diff` | `from?` · `to?`(`git.diff` 의 쪽 — 없으면 작업 트리, 즉 `git diff`) · `path?` · `detail?` · `ignoreWhitespace?` | 바뀐 파일(상태 · 경로 · 더한 · 뺀 줄 수 · 바이너리 · 너무 큼 · 생성 파일). `detail` 이나 `path` 면 파일마다 `patch`(unified diff 글). 예산을 넘으면 그 파일부터 `patch` 를 빼고 `truncated`, 목록도 넘으면 `omittedFiles` |
| `git_blame` | `git.blame`(스트림)의 엔진 | `path` · `rev?` · `startLine` · `endLine`(한 번에 500줄까지) | 범위마다(줄 순서, 같은 커밋이 이어지면 합친다) 커밋 · 그 커밋에서의 경로 · 작성자 · 작성 시각 · 제목, 그리고 blame 한 커밋 · 파일 줄 수 · 엔진. 예산을 넘으면 앞쪽만 싣고 `truncated` 와 싣지 못한 첫 줄 `nextLine`(그 줄을 `startLine` 으로 다시 부른다). 파일 밖 범위 · 그 리비전에 없는 파일은 invalid params |
| `git_branch_list` | `git.refs` | `includeTags?` | 로컬 브랜치(이름 · 해시 · HEAD 여부 · upstream 과 ahead/behind) · 원격 브랜치, `includeTags` 면 태그. 예산을 넘으면 `truncated` |

### 코드 인텔리전스
| 툴 | 설명 | 권한 |
|---|---|---|
| `diagnostics` | LSP 진단 (파일/프로젝트 단위, 심각도 필터) | read |
| `symbol_search` | 워크스페이스 심볼 검색 | read |
| `find_references` / `goto_definition` | 위치 기반 조회 — v1.0 전이다(M5 에 없다 — 06-roadmap.md 결정 D59) | read |

**지금 구현**(16-m5-plan M5-08): 툴 메서드 둘 — `diagnostics` 는 `crates/pw-lsp/src/tools.rs`(`lsp.tool.diagnostics`), `symbol_search` 는 `crates/pw-index/src/tools.rs`(`index.tool.symbolSearch`). 둘 다 `read` · 그룹 `code` 이고 UI 의 RPC 와 같은 함수를 부른다. 앱의 Host 만 처리한다 — 헤드리스 `tools/list` 에는 없다(결정 M5-D3). 위치는 둘 다 줄 · 열 1 부터이고 열은 UTF-16 코드 단위다(밑의 RPC 는 LSP 와 같은 0 부터).

| 툴 | 밑의 함수 | 입력(`projectId` · `worktree?` 말고) | 결과 |
|---|---|---|---|
| `diagnostics` | `lsp.diagnostics.list` 의 저장소(`DiagnosticStore::list`), 서버 요약은 `lsp.status.get` 의 함수(워크트리로 거른 것) | `path?`(절대 경로나 프로젝트 루트 기준 상대 경로) · `minSeverity?`(`error` · `warning` · `information` · `hint` — 그것과 더 심한 것) · `cursor?` · `limit?`(1~5000, 기본 100) | `servers`(인스턴스마다 서버 id · 루트 · 상태 · 진행 — 「오류 없음」 과 「서버가 아직 없다 · 준비 중 · 색인 중」 을 가른다), 경로 차례의 파일마다 진단(심각도 · 줄 · 열 · 끝 줄 · 끝 열 · 메시지 · 코드 · 출처 · 서버 · 설명 링크 · 꼬리표 · 관련 자리), `nextCursor`, 상한에 잘렸으면 `truncated`, 미신뢰면 빈 목록과 `unavailable`(`reason: "untrusted"`) |
| `symbol_search` | `index.symbols.search`(`SymbolIndex::search`) | `query`(공백으로 가른 낱말마다 퍼지) · `kinds?`(없으면 `impl` 을 뺀 모든 종류) · `cursor?` · `limit?`(1~1000, 기본 50) | 잘 맞는 차례로 심볼마다 이름 · 종류 · 컨테이너 · 경로 · 상대 경로 · 언어 · 이름의 줄 · 열 · 정의의 첫 줄 · 끝 줄(화면용 일치 위치 · 점수는 뺀다), 맞은 수 `matched`, `nextCursor`, 색인이 없는 루트 `unindexed` 와 그때의 `hint`(「Pitwall 에서 프로젝트를 열어라」) |

- **관문**: 둘 다 먼저 `agent_grant`(MCP 세션의 연결 범위 · 레지스트리에 있는 프로젝트)를 지난다. UI 의 `lsp.diagnostics.list` · `lsp.status.get` 은 그 프로젝트 창의 연결만 받는다(`exec_grant`) — 툴은 그 관문을 빌리지 않는다. 창 · Host 범위의 호출자는 `PERMISSION_DENIED`(`notAgentSession`), 없는 프로젝트는 그 관문의 invalid params 그대로다. `worktree` 가 그 프로젝트의 루트 · 연결 워크트리가 아니면 둘 다 invalid params 다 — 빈 결과(「서버가 아직 없다」 로 읽힌다)로 받지 않는다
- **서버 · 색인을 만들지 않는다**(결정 M5-D18 · D52 — 미신뢰 · 창 범위 규칙): `diagnostics` 는 그 프로젝트 창이 띄운 언어 서버가 스스로 보낸 진단을 읽기만 한다. 창이 없으면 빈 서버 요약과 빈 목록이다. 열지 않은 파일 전체의 진단 수집은 없다(15-m4-plan.md 「M4 에서 하지 않는 것」) — 툴 설명에 그 한계를 적는다. 미신뢰 프로젝트는 서버가 뜨지 않으므로 빈 목록과 까닭이다. `symbol_search` 의 색인은 창이 `fs.watch` 로 보는 루트에만 있다 — 창이 없으면 `unindexed` 와 `hint` 다. 파싱만 하므로 미신뢰 프로젝트에서도 찾는다(15-m4-plan.md 결정 M4-D8)
- **쪽**: 둘 다 툴 규약의 `cursor` · `limit` · `nextCursor` 다. `symbol_search` 의 커서는 불투명 글자이고 안에 다음 쪽의 시작 자리(`index.symbols.search` 의 `offset`)가 든다 — 입력에 `offset` 을 두지 않는다(16-m5-plan M5-08 ② 메인 판정 2026-11-14)
- **응답 크기**: 툴은 결과 JSON 192 KiB(`pw_lsp::tools::RESPONSE_BUDGET` · `pw_index::tools::RESPONSE_BUDGET`, 세션 상한 256 KiB 아래) 안에서 끊고 그 자리부터 `nextCursor` 로 잇는다. `diagnostics` 는 서버만 되돌려 받는 값(`data`) · 받은 길 · 문서 버전을 싣지 않는다

### 외부 도구
| 툴 | 설명 | 권한 |
|---|---|---|
| `open_with_list` / `open_with_upsert` | 외부 도구 템플릿 관리 | read / write |

`*` UI에 영향을 주지만 비파괴 → 승인 불필요. 창을 앞으로 가져오지 않는다 — 그 프로젝트 창이 열려 있으면 탭만 열고, 없으면 활성화하지 않고 만든다. Host 전체로 호출 빈도를 묶는다(16-m5-plan 결정 M5-D24 · 메인 판정 2026-11-14 — `pitwall tool` 이 호출마다 새 세션이라 세션마다 세지 않는다)

**지금 구현**(16-m5-plan M5-09): `open_with_list`(← `openWith.list` 에 `mcp: true` — 이름 · 등급 · 입력 꼴이 이 표와 같다): 도구 id · 이름 · 기여한 확장 · 파일까지 여는가 · 설정 형식 오류와 기본 도구. 실행 명령은 싣지 않는다. 앱 Host 만(그룹 `open_with`). MCP 세션이 부르면 로그인 셸 환경을 미리 가져오지 않는다(창의 팝업만 곧 실행한다). `open_with_upsert` 는 아직 없다

### 체크아웃 (M7 — 원격과 함께 제공)

한 프로젝트의 체크아웃이 여러 머신에 있을 때(08-remote-mobile.md 「프로젝트와 체크아웃 (D57)」) 에이전트가 대상을 고른다.

| 툴 | 설명 | 권한 |
|---|---|---|
| `checkout_list` | 그 프로젝트의 체크아웃(머신 · 워크트리 · 브랜치)과 머신의 연결 상태 | read |
| `checkout_send` | 한 체크아웃의 브랜치를 다른 머신의 체크아웃으로 보내고 대상을 그 커밋으로 맞춘다(origin 을 거치지 않는다). 원본 · 대상을 받는다 — 가져오기는 둘을 바꿔 준 것이다 | write |

- **대상 지정**: 프로젝트 툴은 선택 입력 `checkout`(머신 + 워크트리)을 받는다. 없으면 세션의 체크아웃(`--project` · cwd 가 가리키는 그 머신의 체크아웃)이다. 선택 입력이라 그 전에 만든 툴과 하위 호환이다 — 프로젝트 툴과 그 RPC 의 입력은 struct 이고 워크트리를 선택 입력으로 받는 꼴을 지킨다(체크아웃을 보지 않고 프로젝트 레코드만 읽는 툴 `project_info` 는 워크트리 입력을 달지 않는다 — 입력이 struct 라 `checkout` 은 깨지 않고 더한다, 16-m5-plan 메인 판정 2026-11-13)
- **다른 머신을 겨눌 수 있는 것은 Desktop 의 세션뿐이다.** 묶음은 Desktop 만 안다 — Desktop 이 묶음을 풀어 그 머신의 Host 로 보낸다. 그 머신에 닿을 수 있으면 그때 연결을 열고(이미 열린 연결이 있어야 하는 것이 아니다), 승인을 물을 자리가 있어야 하므로 그 프로젝트의 창이 열려 있어야 한다. 창이 없거나 헤드리스면 거절한다
- **원격 머신 Relay 의 세션은 그 머신의 체크아웃만 다룬다.** `checkout` 에 다른 머신을 주면 거절하고 `checkout_send` 는 싣지 않는다. 원격 머신의 에이전트가 내 기기에 git 쓰기 · 실행을 하는 길을 열지 않는다 — 원격에서 만든 커밋을 로컬로 옮기는 것은 Desktop 쪽의 가져오기다
- `checkout_send` 는 git 쓰기를 툴로 내놓지 않는다는 규칙(「Git」 표)의 예외다. 에이전트가 git CLI 로는 다른 머신에 origin 없이 보낼 길이 없다. git 이 거절하면(갈라짐 · 대상의 커밋 안 한 변경) 덮지 않고 그 원문을 툴 오류로 돌려준다
- 로컬에서 쓰고 원격에서 테스트하는 흐름: 커밋 → `checkout_send` → 대상 체크아웃으로 `run_start` → `run_logs`. CLI 도 같다(`pitwall tool run_start --checkout …`)
- 승인은 「권한 모델」 의 「다른 머신을 겨누는 호출」, 감사 로그는 호출을 받은 쪽에 남고 인자 요약에 대상 체크아웃이 든다

### 툴 레지스트리 (RPC → 툴)

툴은 손으로 등록하지 않고 RPC 선언(`rpc_method!`)에서 만든다. 구현: `crates/pw-mcp/src/catalog.rs`.

- **노출은 선언이 고른다**: `rpc_method!` 의 선택 필드 `mcp`(`mcp: true` 또는 `mcp: { group, name }`)를 단 메서드만 툴이 된다. 생략하면 꺼짐 — **새 RPC 는 말없이 툴이 되지 않는다**(UI 전용 메서드 `host.demoTask` · `project.markOpened` · `i18n.setLanguage` 등). 툴이 되는 메서드의 고정 목록은 모든 메서드 크레이트가 링크되는 데스크톱 크레이트의 `mcp_tools_are_exactly_these`(`apps/desktop/src-tauri/src/mcp_tests.rs`)가 Host 종류별 목록 둘(앱 · 헤드리스 — 헤드리스는 앱의 부분집합)로 지킨다 — 어느 크레이트의 선언에 `mcp` 를 달아도 거기서 실패하므로, 툴을 늘리려면 선언과 그 목록을 함께 고친다. 스트림 메서드에는 달 수 없다(컴파일 오류)
- **세션의 툴은 그 Host 가 처리하는 것만**: 카탈로그는 실행 파일에 링크된 선언에서 만들고, 세션을 여는 Host 의 라우터가 가진 메서드로 거른다. 헤드리스 Host 는 코어 메서드만 등록하므로 다른 크레이트의 툴이 생겨도 헤드리스 `tools/list` 에는 나오지 않는다
- **role 은 노출을 정하지 않는다**: role 은 「어느 기기가 부를 수 있나」다. MCP 세션은 Host 에 **owner** 로 붙고(로컬 소켓 · stdio 로만 오는 이 사용자의 프로세스다), 부를 수 있는지는 「툴 노출 설정」 → 「권한 모델」 절이 정한다. 세션은 **제 연결 범위**로 붙는다 — 창 범위가 필요한 RPC(쓰기 · 실행 · DB)는 MCP 세션을 거절하고, 툴 메서드만 MCP 세션용 관문을 받는다([03-architecture.md](03-architecture.md) 「RPC 설계」 의 연결 범위).
- **이름**: 선언이 `mcp` 에 이름을 적으면 그 이름, `mcp: true` 면 RPC 이름의 `.` → `_`, camelCase → snake_case(`project.list` → `project_list`, `project.markOpened` → `project_mark_opened`). `[a-z0-9_]` 64자 이내, 겹치면(적은 이름이 다른 선언의 규칙 이름과 겹쳐도) 시작 시 실패
- **그룹**: 선언의 `mcp` 가 툴 이름과 함께 싣는다(`mcp: { group: "code", name: "diagnostics" }`). `mcp: true` 면 RPC 이름의 첫 마디를 snake_case 로 옮긴 것이다(`openWith` → `open_with`). 「툴 노출 설정」 의 그룹 이름(`pw_mcp::catalog::GROUPS`)만 받고, 그 밖이면 시작 시 실패
- **툴 메서드**: 이 문서의 툴과 이름 · 등급 · 꼴이 다른 RPC 에는 `mcp` 를 달지 않고 그 기능 크레이트에 툴 메서드를 따로 선언한다 — 같은 내부 함수 · 같은 관문을 부르는 얇은 선언이다(16-m5-plan 결정 M5-D26). UI 가 쓰는 RPC 는 화면에 맞춘 꼴이고 등급도 다르다. 툴의 등급은 그 선언의 `tier` 이고, 감사 행에 남길 상세는 결과 · 오류 `data` 의 최상위 `audit` 글로 싣는다(「감사 로그」 의 「툴 메서드 규약 — 감사 상세」), 프로젝트 툴의 입력은 struct 에 선택 입력 `worktree` 를 둔다(「체크아웃」 — 프로젝트 레코드만 읽는 툴은 뺀다). 쪽 단위 툴은 선택 입력 `cursor` · `limit` 을 둘 다 받고 결과에 `nextCursor`(없으면 마지막 쪽)를 싣는다. 규약은 `crates/pw-mcp` 머리 주석, 시험은 `tools_follow_the_tool_method_convention`
- **설명 · 등급**: 선언의 문서 주석과 `tier`. `read` 툴에는 `readOnlyHint` 를 단다
- **입력 스키마**: 입력 타입의 schemars 스키마를 루트로 펼친다. struct 만 툴이 된다(MCP `inputSchema` 는 object). `()` 입력은 빈 object 로 내보내고 RPC 에는 `null` 을 보낸다. enum 루트는 툴로 만들 수 없다
- **출력**: 결과 스키마의 루트가 object 면 `outputSchema` 와 `structuredContent`, 아니면(배열 등) JSON 텍스트만
- **결과 표지**: `tools/call` 결과의 `_meta` 에 결과(`ok` · `error` · `denied`)와 까닭 코드를 싣는다(취소는 거절이 아니라 결과 `error` 의 까닭 `cancelled` 다 — 감사 로그와 같다). 키는 `pitwall/outcome`(감사 로그의 `outcome` 과 같은 셋)과 `pitwall/reason`(까닭이 없으면 키가 없다). 노출 안 된 툴 호출(JSON-RPC invalid params)은 오류의 `data` 객체에 같은 두 키를 싣는다. CLI 의 종료 코드가 이것을 읽는다(「CLI」). 구현과 읽는 쪽: `pw_mcp::marker`(`Mark::read` — 모르는 까닭 코드는 까닭 없음으로 읽어 결과 값으로는 계속 가른다). 다른 행이 코드를 더하면 아래 표에 한 줄을 더한다

  | 까닭 코드 | 결과 | 뜻 | 지금 내는 곳 |
  |---|---|---|---|
  | `not_exposed` | `denied` | 세션 또는 대상 프로젝트의 노출 설정이 그 툴을 껐다(모르는 툴 이름 포함) | 세션의 노출(invalid params) · 대상 프로젝트의 노출 |
  | `needs_approval` | `denied` | 사람의 승인이 있어야 하는데 받을 길이 없다 | 앱 세션의 `write` 이상 |
  | `approval_denied` | `denied` | 승인 창에서 거절됐다 | 아직 없다(16-m5-plan M5-12) |
  | `approval_timeout` | `denied` | 승인 창이 시간 안에 답을 받지 못했다 | 아직 없다(M5-12) |
  | `needs_app` | `denied` | 앱이 있어야 하는 툴을 헤드리스에서 불렀다(결정 M5-D3) | CLI(`pitwall tool`)가 부르지 않고 가른다 — 실행 파일에 링크된 카탈로그에는 있고 헤드리스 Host 의 카탈로그에는 없는 이름이면 종료 코드 3(「CLI」 절). 서버는 아직 내지 않는다 — 헤드리스 MCP 세션에서 앱 전용 이름은 `not_exposed` 이고, 그 에이전트에게는 서버 안내문(`HEADLESS_NOTICE`)이 같은 뜻을 전한다. 서버가 `needs_app` 을 내는 것은 16-m5-plan M5-20(헤드리스에서 앱으로 넘겨 붙기)이 정한다 |
  | `headless_write` | `denied` | 헤드리스에서 `write` 이상을 불렀다(승인 경로가 없다) | 헤드리스 세션의 `write` 이상 |
  | `cancelled` | `error` | 호출이 취소됐다(취소 알림 · 세션 끊김 · Host 가 그 RPC 를 거둠) | 「호출 취소」 |
  | `too_large` | `error` | 응답이 한 건 상한(256 KiB)을 넘었다 — 범위를 좁혀 다시 부른다 | 「응답 크기」 |
  | `permission_denied` | `denied` | 툴의 Host RPC 가 `PERMISSION_DENIED` 로 답했고 그 `data.reason` 이 아래 코드가 아니다(없음 포함) | 툴의 관문 |
  | `not_opened_by_user` | `denied` | 사람이 이번 앱 실행에서 Pitwall 의 DB 탐색기로 그 연결(같은 대상)을 연 적이 없다(`data.reason = "notOpenedByUser"`) | DB 툴의 관문(16-m5-plan M5-06 — 「DB」 의 「에이전트의 DB 연결」) |
  | `untrusted` | `denied` | 프로젝트를 신뢰하지 않았다(`data.reason = "untrusted"`) | DB 툴의 관문 |
  | `production_blocked` | `denied` | 프로덕션 표지가 있는 DB 대상이다(`data.reason = "productionBlocked"`) | DB 툴의 관문 |
  | `rate_limited` | `denied` | Host 전체의 호출 빈도를 넘었다 — 잠시 뒤 다시 부른다(`data.reason = "rateLimited"`) | `project_open`(16-m5-plan M5-09 — 「프로젝트」 표 아래 「지금 구현」) |

  **툴의 Host RPC 가 `PERMISSION_DENIED` 로 답하면 결과는 `denied` 다**(메인 판정 — 16-m5-plan M5-06): 까닭은 `data.reason`(camelCase)을 위 표의 코드로 옮긴 것이고(대응은 `pw_mcp::marker::Reason::of_permission_denied` 한 곳), 모르거나 없으면 `permission_denied` 다. 거절 까닭을 더하는 행은 `Reason` 과 그 대응에 한 줄씩 더하고 이 표에 한 줄을 더한다. `PERMISSION_DENIED` 가 아닌 오류(비밀 · 확인이 필요하다는 DB 의 3101 · 3102 등)는 `error` 다. 까닭이 없는 `error` 는 툴(Host RPC)이 실패한 것이다
- **응답 크기**: 툴 응답 한 건은 직렬화한 결과 JSON 256 KiB 까지(`pw_mcp::server::MAX_RESPONSE_BYTES`), 목록 툴의 `limit` 기본은 100(`DEFAULT_LIST_LIMIT` — `git_log` · `symbol_search` 50, `git_status` 200. 16-m5-plan 결정 M5-D34, 수는 출발값). 툴 메서드는 넘지 않게 자르고(`truncated` 와 다음 `cursor`), 그래도 넘은 응답은 세션이 결과 대신 「범위를 좁혀라」 오류(까닭 `too_large`)로 바꾼다. 재는 것은 Host RPC 의 결과 값이다 — object 결과는 `structuredContent` 와 `content` 의 텍스트로 두 번 실려 나가므로 클라이언트가 받는 글은 그 두 배를 넘을 수 있다. 감사 행은 결과 `error` 이고 크기 열에 넘은 크기를 남긴다
- **프로젝트 지정**: 입력에 `projectId` 가 있으면 프로젝트 툴이다. 세션에 프로젝트가 있으면(`--project` · cwd) `projectId` 를 필수에서 빼고 빠지면 서버가 채운다. 입력에 `worktree` 가 있고 비었으면(없음 · `null` · 빈 글자열) 세션의 활성 워크트리 꼭대기(`project.resolve` 의 `worktree.top`)로 채운다(세션을 연 경로가 미등록 워크트리 안일 때 — Orca 워크트리 안의 에이전트가 메인 체크아웃이 아니라 자기 워크트리를 본다). 준 값은 덮지 않고, 호출이 세션 프로젝트가 아닌 `projectId` 를 겨누면 채우지 않는다 — 세션의 워크트리는 세션 프로젝트의 것이다
- **지금 툴**: `project_list`(← `project.list`), `project_info`(← `project.info`: 프로젝트 · 루트 폴더 유무 · 빌드 파일 · 빌드 파일에서 짐작한 언어 · 설정 폴더와 그 안의 설정 파일 id. 설정 내용은 읽지 않는다 — 읽으면 구버전 파일의 마이그레이션이 따라와 조회 툴이 저장소를 고친다), `git_status` · `git_log` · `git_diff` · `git_blame` · `git_branch_list`(← `git.tool.*` 툴 메서드, 앱 Host 만 — 「Git」 절), `db_connection_list`(← 툴 메서드 `db.tool.connectionList`) · `db_schema`(← 툴 메서드 `db.tool.schema`) · `db_query`(← 툴 메서드 `db.tool.query`) · `db_kv_scan`(← 툴 메서드 `db.tool.kvScan`) · `db_kv_get`(← 툴 메서드 `db.tool.kvGet`) — 다섯 다 앱 Host 만, 「DB」, `settings_get`(← 툴 메서드 `config.tool.get`, 코어 툴) · `open_with_list`(← `openWith.list`, 앱 Host 만) · `project_open`(← 툴 메서드 `project.tool.open`, 앱 Host 만 — 「프로젝트」 · 「외부 도구」 표 아래 「지금 구현」)
- MCP 메시지(툴 설명 · 오류)는 번역하지 않는다
- **툴 설명 · 스키마 설명 · 툴의 오류 문장은 영어다**(사용자 결정 2026-11-17 — 에이전트가 읽는 글, 16-m5-plan 결정 M5-D39). 입출력 스키마의 설명은 `rpc_method!` 선언과 입출력 타입(그 타입이 쓰는 타입 포함)의 문서 주석에서 오므로 그 문서 주석을 영어로 쓴다. `apps/desktop/src-tauri/src/mcp_tests.rs` 의 `tool_descriptions_and_schemas_are_english` 가 툴 설명 · 입력 · 출력 스키마의 한글을 잡는다 — 툴이 런타임에 내는 오류 · 안내 글은 이 시험이 보지 않는다

## 3. 권한 모델

| 등급 | 예 | 기본 정책 |
|---|---|---|
| `read` | 조회 전부 | 자동 허용 |
| `write` | 설정 파일 변경 | **UI 승인** (diff 미리보기). "이 세션 동안 허용" 옵션 |
| `exec` | 프로세스 실행 | 신뢰된 프로젝트 + 이미 존재하는 구성: 자동 / 방금 생성·수정된 구성: 승인 (`sql` 종류 구성은 이 줄이 아니라 실행 시점의 SQL 분류를 따른다 — 「실행 구성」 표 아래) |
| `dangerous` | DB 쓰기 쿼리, 프로덕션 태그 연결 접근 | 매번 승인, 프로덕션은 기본 차단 |

- 정책은 **사람의 설정(전역 · `.pitwall/*.local.toml` · 개인 전용 `project.toml`)** 에서 조정한다. `[mcp.policy]` 의 키는 둘이다 — `approval_timeout_secs`(승인 시한, 기본 300), `production`(프로덕션 태그 연결 — `block` 기본 · `ask`. 「DB」 표 아래 「에이전트의 DB 연결」). **커밋되는 공유 설정(`.pitwall/project.toml`)의 `[mcp.policy]` 는 읽지 않는다** — 효과가 없고 경고를 남긴다. 저장소를 클론해 연 것만으로 프로덕션 차단이 「매번 승인」 으로 풀리거나, 승인 시한이 줄어 모든 승인이 거절되는 일이 없게 한다. 그래서 `production = "ask"` 는 사람의 설정에서만 켠다(16-m5-plan 결정 M5-D31)
- **「이 세션 동안 허용」** 은 그 세션 · 그 프로젝트 · 그 툴에 한한다. `open_with_upsert` 에는 없다 — 명령을 정의하는 쓰기라 정의마다 사람이 본다. CLI 호출의 승인에서는 그 자리가 「이 프로젝트 창이 열려 있는 동안 CLI 허용」 이다(「CLI」)
- **승인은 계획의 해시에 묶는다**: 승인을 청하는 툴 메서드는 실행 전에 계획(등급 · 요약 · 바뀔 것 — 파일 diff 나 SQL 문장들)을 만들고, 승인은 그 해시에 묶인다. 해시는 툴 이름 · 서버가 채운 뒤의 입력 전체 · 툴이 더하는 대상 값(DB 는 연결의 푼 대상 · 방언 · 프로덕션 · read-only 표지, 파일을 고치는 툴은 고치기 전 파일 내용)을 덮는다. 실행 때 계획을 다시 만들어 해시가 다르면 돌리지 않는다. 해시를 싣는 자리는 툴 입력 스키마에 없다 — 에이전트가 승인을 스스로 실을 수 없다
- **다른 머신을 겨누는 호출**(「체크아웃」 의 `checkout` 입력 — M7): 등급 규칙은 같고 `exec` 의 「신뢰된 프로젝트」 는 대상 머신의 신뢰를 본다. **`auto_approve` 는 다른 머신을 겨누는 호출에 적용하지 않는다.** 그 세션에서 어느 머신을 처음 겨누는 `write` · `exec` 호출은 승인을 묻고, 사람이 「이 세션 동안 이 머신 허용」 을 고르면 그 세션의 그 머신에 한해 다시 묻지 않는다(CLI 호출에서는 「CLI」 의 창 단위 허용과 같은 꼴이다 — 06-roadmap.md 결정 D61). 대상이 미신뢰 체크아웃이면 세션 허용과 상관없이 매번 묻는다. `dangerous` 는 매번이다
  - **Why:** 로컬에서 쓰고 원격에서 테스트하는 되풀이를 에이전트가 하면 매번 승인은 쓸 수 없고, 로컬과 같은 자동 승인은 사람이 본 적 없는 커밋이 승인 없이 다른 머신에서 도는 길을 연다. 미신뢰 프로젝트에서도 git 쓰기가 되는 근거는 「사람이 누른 것」(06-roadmap.md 결정 D43)이고 에이전트의 호출에서는 승인이 그 자리를 맡는다
- **지금 구현**(`crates/pw-mcp/src/policy.rs`): `read` 는 허용, `write` 이상은 승인 화면이 아직 없어 **거절**한다(이유를 툴 오류로 돌려주고 감사 로그에 `denied`). `[mcp.policy]` · `auto_approve` 는 아직 읽지 않는다. 승인 경로 · 계획의 해시 · 「이 세션 동안 허용」 은 16-m5-plan M5-12 가 만든다
- 모든 MCP 호출은 감사 로그에 기록 → UI "에이전트 활동" 패널

### 감사 로그

- **한 파일 `$STATE/mcp/audit.db`(SQLite)** 에 `project_id` 열을 둔다. 프로젝트를 정하지 않는 호출(`project_list`)은 `project_id` 가 `NULL` 이다. 프로젝트마다 파일을 두지 않는 이유: 어느 프로젝트에도 속하지 않는 호출이 있고, `$STATE` 가 감사 기록의 자리이며([03-architecture.md](03-architecture.md) 「데이터 저장」 표), 보관 기간 정리가 한 곳에서 끝난다. 「에이전트 활동」 패널은 `project_id` 로 거른다
- 파일 0600, 새로 만드는 폴더 0700(이미 있는 폴더의 권한은 바꾸지 않는다). 툴 이름은 64자, 오류 · 거절 이유는 512자까지 남긴다(노출 안 된 툴 호출은 클라이언트가 보낸 이름을 그대로 남기므로)
- 한 행 = `tools/call` 하나: 시각 · 세션 id · 클라이언트(`clientInfo` 이름 버전) · 프로젝트 · 툴 · 인자 요약 · 결과(`ok` · `error` · `denied`) · 오류나 거절 이유 · 결과 크기(바이트) · 소요(ms). **결과 내용은 남기지 않는다.** 노출되지 않은 툴 호출도 `denied` 로 남는다. `write` 이상의 호출은 실행 전에 행을 `started` 로 쓰고 끝나면 같은 행을 고친다 — 끝을 못 쓴 행은 `started` 로 남는다. 승인을 지난 호출은 승인 방식(`auto` · `once` · `session` · `window`)과 계획 해시를 남긴다(둘 다 아직 없다 — 16-m5-plan M5-11 · M5-12)
- **인자 요약**: 키를 단어로 갈라(camelCase · `_` · `-`) `password` · `secret` · `token` · `key` · `auth` · `credential` · `cookie` · `private` 등이 있으면 값을 `[redacted]` 로 바꾼다. 글자열은 80자, 배열은 8개, 깊이 3, 전체 1024바이트까지. 키 이름으로 못 거르는 비밀이 남을 수 있으므로 비밀을 툴 인자로 받지 않는다는 규칙(`db_connection_upsert`)이 첫 방어선이다. 목록은 `pw_mcp::audit::SECRET_WORDS`. `db_query` · `db_execute` 는 SQL 원문 대신 문장 수 · 가장 높은 등급 · 문장 해시(`file` 이면 경로와 내용 해시)를 상세 칸에 남긴다 — 글자열 상한이 원문을 자르기 때문이다(16-m5-plan M5-07. 꼴은 `statements=2 grade=read sql=sha256:… file=queries/a.sql content=sha256:…`). `sql` 은 인자 요약에서도 「원문 키」 라 값 대신 글자 수(`[<n> chars]`)만 남는다(`pw_mcp::audit::CONTENT_WORDS` — 키를 단어로 갈라 `sql` 이 있으면. 비밀 키이기도 하면 `[redacted]` 가 앞선다. 16-m5-plan M5-07 리뷰 F1). **툴 메서드 규약 — 감사 상세**(메인 판정 2026-11-14): 툴이 결과(object)의 최상위 `audit` 칸에 글을 실으면 세션이 그 글을 성공 행의 상세 칸에 넣고, 오류 · 거절이면 오류 `data`(object)의 최상위 `audit` 을 「오류 글 · 상세」 로 넣는다(그때 `data` 를 통째로 싣지 않는다 — 서버 오류 글의 값이 섞인다). 상세 상한 512자에서 잘린다. 결과에 남으므로 에이전트에게도 보인다 — 원문 · 값 · 비밀을 넣지 않는다(`pw_mcp::server::AUDIT`, `crates/pw-mcp` 머리 주석). 칸이 없는 툴은 성공 행의 상세가 비고 오류 행은 오류 글 그대로다
- 여러 프로세스(앱, 헤드리스 `pitwall mcp` 여럿)가 같은 파일에 쓴다(`BEGIN IMMEDIATE`, 2초 대기). 기록이 실패해도 `read` 호출은 실패시키지 않고 경고 로그를 남긴다. **`write` 이상은 감사 행을 먼저 쓰고 못 쓰면 실행하지 않는다.** **보관은 기본 90일**(`[mcp.audit] retention_days`, 0 은 지우지 않음)이고 「에이전트 활동」 패널에서 지울 수 있다(06-roadmap.md 결정 D62). **`[mcp.audit]` 표는 전역 설정에서만 읽는다** — 감사 파일이 하나다. 프로젝트 설정(공유든 개인이든)의 그 표는 효과가 없고 경고를 남긴다. 선기록과 보관 정리는 아직 없다(16-m5-plan M5-11) — `write` 이상의 툴이 아직 없어 지금 기록은 모두 호출이 끝난 뒤의 한 번이고, 지우는 것도 없다
- **읽기 — 지금 구현**(16-m5-plan M5-10): `mcp.audit.list { projectId?, cursor?, limit?, outcome?, tool? }`(read, owner — 에이전트 범위 연결은 거절한다). 최신부터 쪽 단위(`limit` 1~500, 기본 100, 커서는 행 id — `nextCursor` 가 `null` 이면 마지막 쪽)이고, `projectId` 를 주면 그 프로젝트의 행과 프로젝트 없는 행이다(「에이전트 활동」 패널이 보이는 것). 앱 안의 세션이 쓴 행은 실제로 쓰인 뒤 `mcp.activity`(그 프로젝트 범위, 프로젝트 없는 호출은 Host 범위 — payload 는 행 id 와 `projectId` 뿐)를 낸다. 쓰기가 실패하면 내지 않는다. 다른 프로세스(헤드리스 `pitwall mcp` · `pitwall tool`)가 쓴 행은 이벤트가 없다 — 「에이전트 활동」 패널(M5-13)이 열릴 때 · 창이 앞으로 올 때 다시 읽는다
- **승인 요청 라우팅**: 해당 프로젝트 창이 열린 Desktop에 다이얼로그 + OS 알림. 원격 Relay에서 온 요청도 연결된 Desktop으로 전달. 타임아웃(기본 5분) 시 거절. 그 프로젝트 창이 없으면 기다리지 않고 거절한다. 프로젝트가 없는 쓰기(`project_register` · `open_with_upsert`)는 가장 최근에 앞에 있던 Pitwall 창에 묻고(다이얼로그에 그 프로젝트의 요청이 아님을 보인다), 창이 없으면 거절한다. (모바일 승인은 모바일 보류에 따라 제외)
- 원격 머신에서 도는 에이전트도 해당 Relay의 MCP 엔드포인트를 사용 → 같은 승인 흐름
- 확장(WASM 플러그인)이 추가한 MCP 툴은 `<ext-id>.<tool>` 이름으로 노출, 권한 등급은 확장 매니페스트에 선언

## 3-1. 툴 노출 설정 (툴별 on/off)

JetBrains IDE 내장 MCP 서버의 "Exposed Tools" 방식을 따른다. 권한 등급(3절)과 별개로 **툴 자체를 에이전트에게 보이지 않게** 할 수 있다.

| 항목 | 설계 |
|---|---|
| 단위 | 툴 단위 on/off (꺼진 툴은 `tools/list`에 나오지 않음 → 에이전트 컨텍스트 토큰도 절약) |
| 그룹 | 모듈 그룹(project / run / debug / db / git / code / open_with / 확장별) 일괄 on/off — `[mcp.groups]`(그룹 이름 = 켬 · 끔). 그룹은 툴의 선언이 싣는다(「툴 레지스트리」) |
| 프리셋 | `read-only`(조회 툴만) · `standard`(기본: 조회 + 설정) · `full`(실행 · DB 쓰기 포함) |
| 범위 | 전역 기본값 → 프로젝트별 덮어쓰기 (`.pitwall/project.toml` 또는 `.local.toml`, 개인 전용 프로젝트는 `$CONFIG/projects/<slug>/project.toml`). **커밋되는 공유 설정(`.pitwall/project.toml`)은 노출을 좁히기만 한다** — 프리셋을 넓히거나 꺼진 툴을 켜는 것은 전역 · `.pitwall/*.local.toml` · 개인 전용 설정만 한다. 저장소를 클론한 것만으로 에이전트에게 열리는 툴이 늘지 않게 한다. `auto_approve` 와 그룹 키도 같다 — 공유 설정은 자동 승인을 더하거나 그룹을 켜지 못한다. `[mcp.policy]` 는 공유 설정에서 아예 읽지 않는다(「권한 모델」). `[mcp.audit]` 는 전역 설정에서만 읽는다(「감사 로그」) |
| 자동 승인 | 등급별 "확인 없이 실행" 토글 (JetBrains의 brave mode에 해당). `dangerous` 등급은 자동 승인 불가. `open_with_upsert` 와 「이 연결 열기」 승인(「DB」)에도 적용하지 않는다 |
| 변경 반영 | 설정 변경 시 MCP `notifications/tools/list_changed` 전송 → 연결된 에이전트가 즉시 갱신 |
| 확장 툴 | 확장이 추가한 툴은 **기본 꺼짐**, 사용자가 켜야 노출 |
| UI | 설정 > MCP: 툴 목록(이름 · 설명 · 등급 · 최근 호출 수), 검색, 그룹 토글, 입력 스키마 미리보기 |

지금 구현(`crates/pw-mcp/src/exposure.rs`):
- 읽는 키는 `preset` · `[mcp.tools]` · `[mcp.groups]` 다. 전역 `[mcp]` 위에 프로젝트 `[mcp]` 를 설정 계층의 병합 규칙으로 덮는다(`[mcp.tools]` · `[mcp.groups]` 는 키별로 합쳐진다). 내장 기본값은 `preset = "standard"`
- 「공유 설정은 좁히기만」: 저장소 모드 프로젝트는 공유 레이어까지 읽은 노출과, 공유 레이어를 뺀(전역 + `.local.toml`) 노출이 **둘 다 켠 툴**만 켠다(`Exposure::narrowed_by`). 개인 전용 프로젝트의 `project.toml` 은 개인 설정이라 넓힐 수 있다
- `tools/list` 는 **세션 프로젝트**의 설정으로 정한다. 호출은 세션의 노출과 **대상 프로젝트의 노출**(프로젝트 툴의 `projectId` 가 세션 프로젝트가 아닐 때 그 프로젝트의 `[mcp]` — 공유 레이어는 좁히기만)을 둘 다 지나야 한다(16-m5-plan 결정 M5-D28, `pw_mcp::server` 의 `Session::call`). 대상 프로젝트가 끈 툴은 결과 `denied` · 까닭 `not_exposed` 다. 대상 프로젝트의 설정은 호출마다 **디스크에 쓰지 않고** 읽는다(`ConfigSource::without_writes`). `auto_approve` 와 승인 창은 대상 프로젝트의 것으로 판정한다 — 승인 경로와 함께 아직 없다(M5-12)
- 프리셋은 등급으로 고른다: `read-only` = `read`, `standard` = `read` · `write`, `full` = 전부
- 판정 순서: 툴 이름과 정확히 같은 키 → 맞는 glob(`*` 는 아무 글자열) 중 가장 긴 것 → 그룹(`[mcp.groups]`) → 프리셋
- 그룹은 툴의 선언이 싣는 칸이다(「툴 레지스트리」). glob 은 접두사 없는 이름(`diagnostics` · `settings_get`)을 놓치고 꺼 둔 그룹에 뒤에 더해진 툴을 켜진 채 내보내지만, 그룹 키는 그 그룹의 툴을 이름과 상관없이 모두 끈다. 공유 설정은 그룹 키로도 좁히기만 한다(같은 `Exposure::narrowed_by`)
- 잘못된 값은 버리고 경고 로그를 남긴다(모르는 그룹 이름, bool 이 아닌 툴 · 그룹 값). 모르는 프리셋은 가장 좁은 `read-only` 로 본다
- 변경 반영: 전역 · 세션 프로젝트의 설정 파일을 감시해 **보이는 툴 집합이 바뀌면** `notifications/tools/list_changed` 를 보낸다. 2025-11-25 까지의 클라이언트는 initialize 뒤 세션 알림으로, 2026-07-28 클라이언트는 `subscriptions/listen` 구독으로 받는다
- 판정은 켜졌는지와 **그것을 정한 규칙**을 함께 낸다(`Exposure::verdict` — `enabled` 는 이것의 값이다): `preset` · `tool`(정확한 키) · `glob` · `group` · `shared`(공유 레이어를 빼면 켜지는데 공유 설정이 껐다 — 공유 설정이 켜려다 상한에 막힌 툴은 상한의 규칙이다)와 그 키. 사람의 화면은 `mcp.tools.list { projectId? }`(read, owner — 16-m5-plan M5-10)로 카탈로그 전부(이름 · 설명 · 등급 · 그룹 · 입력 스키마)와 그 판정, 최근 30일 호출 수를 읽는다. `projectId` 를 주면 그 프로젝트의 설정(대상 프로젝트 판정과 같은 읽기 — `exposure::of_project`)과 그 프로젝트의 호출 수, 없으면 전역 설정과 전체 호출 수다. `mcp.*` RPC 에는 `mcp` 를 달지 않는다 — 에이전트가 자기 감사 로그 · 노출 설정을 읽는 툴은 없다. 툴 목록에 없는 것만으로 막지 않는다: `mcp.*` 는 모두 **에이전트 범위 연결(MCP 세션이 Host 에 붙는 연결)을 거절한다**(`PERMISSION_DENIED`, `data.reason = "agentSession"` — `project.setTrust` 와 같은 꼴, `pw_mcp::methods::WindowsOnly` 한 자리가 거절하고 뒤 행이 더하는 `mcp.*` 메서드도 그 자리를 지난다). 단일 파일 창 연결도 부르지 못한다
- `auto_approve` · 확장 툴 기본 꺼짐 · 설정 > MCP 화면은 아직 없다

```toml
# ~/.config/pitwall/settings.toml (전역) — 프로젝트에서 같은 키로 덮어쓰기 ([mcp.policy] · [mcp.audit] 는 아래 주석)
[mcp]
preset = "standard"
auto_approve = ["read", "write"]      # "exec" 추가 가능, "dangerous"는 불가

[mcp.tools]
db_query = true
run_start = false                      # 개별 툴 끄기
"dev.pitwall.ext-kafka.*" = true       # 확장 툴 켜기 (glob)

[mcp.groups]
db = false                             # 그룹 끄기 (project / run / debug / db / git / code / open_with)

# [mcp.policy] 는 전역 · .pitwall/*.local.toml · 개인 전용 project.toml 에서만 읽는다
# (커밋되는 .pitwall/project.toml 의 [mcp.policy] 는 효과가 없고 경고를 남긴다)
[mcp.policy]
approval_timeout_secs = 300            # 승인 시한
production = "block"                   # "ask" 면 사람이 연 프로덕션 연결을 매번 승인으로

# [mcp.audit] 는 전역 설정에서만 읽는다 — 감사 파일이 하나다
# (프로젝트 설정의 [mcp.audit] 는 공유든 개인이든 효과가 없고 경고를 남긴다)
[mcp.audit]
retention_days = 90                    # 감사 로그 보관. 0 은 지우지 않음
```

### 클라이언트 자동 등록 (F10-5)
- P1 이다(v1.0 전). 그때까지는 설정 > MCP 화면이 클라이언트별 등록 명령을 복사할 수 있게 보인다(16-m5-plan M5-15)
- 전역 등록: Claude Code(`~/.claude.json`), Cursor, Codex 등 감지된 클라이언트 설정에 `pitwall mcp` 항목 추가
- 프로젝트 등록: 프로젝트 루트의 `.mcp.json` 등에 추가 → 해당 프로젝트에서만 Pitwall 툴 노출
- 등록 전 변경 diff를 보여주고 승인 후 기록, 해제(제거)도 같은 화면에서

## 3-2. CLI (`pitwall tool`)

MCP 툴 전부를 같은 이름 · 같은 입력 · 같은 판정으로 명령줄에서도 부른다(06-roadmap.md 결정 D54). MCP 클라이언트가 없는 에이전트 · 셸 스크립트 · 사람이 쓰고, 툴 스키마를 컨텍스트에 싣지 않고 필요한 툴만 부르는 길이기도 하다.

구현: `apps/desktop/src-tauri/src/tool_cli.rs`(명령 · 플래그 · 종료 코드 · MCP 클라이언트), `mcp.rs`(앱 소켓에 붙기 `connect_app` · 헤드리스 세션 `Headless` — `pitwall mcp` 와 함께 쓴다).

| 명령 | 하는 일 |
|---|---|
| `pitwall tool list` | 이 세션에 노출된 툴의 이름 · 등급 · 설명 한 줄. 꺼진 툴은 나오지 않는다(`tools/list` 와 같은 집합) |
| `pitwall tool <툴> --help` | 그 툴의 설명 · 등급 · 입력 스키마(플래그 이름 · 타입 · 필수 여부) |
| `pitwall tool <툴> [--<키> <값>]... [--input <json>\|-]` | 툴 호출. 결과를 stdout 에 JSON 문서 하나로 쓴다 |

- **CLI 는 로컬 소켓에 붙는 MCP 클라이언트다.** `pitwall tool` 은 「로컬 소켓과 헤드리스」 절의 `pitwall mcp` 와 같은 길로 세션을 열고(소켓이 있으면 앱에, 없으면 같은 프로세스의 헤드리스 Host 에 프로세스 안 duplex 로) `initialize` → `tools/list` → `tools/call` 을 보낸다. 서버 쪽에 CLI 전용 경로를 두지 않는다 — 카탈로그 · 노출 · 권한 · 감사 로그가 MCP 와 한 벌이다. CLI 에만 있는 툴도, MCP 에만 있는 툴도 없다. 클라이언트는 rmcp 의 `client` 기능이다(서버와 같은 구현 — 버전 협상 · 취소 알림 · 쪽 넘기기)
  - **Why:** 경로가 둘이면 툴을 끈 설정 · 승인 정책을 CLI 로 돌아갈 수 있고, 두 판정이 조용히 갈린다(02-tech-stack.md 「DB 계층」 의 DBX 비교 — CLI 가 MCP 와 다른 쓰기 판정을 쓴다)
- **툴 이름**은 MCP 이름 그대로다(`pitwall tool git_status`). 「툴 레지스트리」 절의 규칙으로 RPC 선언에서 나오므로 툴이 늘면 CLI 도 함께 는다. 손으로 쓰는 서브커맨드 표는 없다. 툴 이름 `list` 는 명령과 겹쳐 쓰지 못한다(예약 플래그와 같은 카탈로그 시험이 막는다)
- **등급**(`list` · `--help` 의 `read` · `write` …)은 실행 파일에 링크된 카탈로그에서 읽는다 — MCP `tools/list` 에는 등급이 없다(`readOnlyHint` 뿐). 이 실행 파일이 모르는 툴이면 `readOnlyHint` 로 `read` 만 알고 나머지는 `?`
- **프로젝트**: `--project <path>`, 없으면 cwd 로 해석한다(「전송 방식」 절과 같은 규칙). 세션에 프로젝트가 있으면 `projectId` 는 서버가 채운다
- **입력**: 입력 스키마의 최상위 필드마다 플래그 하나(camelCase → kebab-case, `maxCount` → `--max-count`). 값은 스키마의 타입으로 읽는다 — 글자열은 그대로, 수 · 불리언 · 배열 · 객체는 JSON 으로. `--input <json>` 은 입력 객체 전체이고 `--input -` 는 stdin 에서 읽는다. 둘을 함께 주면 플래그가 덮는다. 스키마에 없는 플래그 · 타입이 안 맞는 값 · 빠진 필수 필드(세션 프로젝트가 채우는 `projectId` 는 빼고)는 호출하지 않고 사용법 오류다. 글자열과 다른 타입을 함께 받는 필드(`anyOf`)는 JSON 으로 읽히는 값이면 그 값, 아니면 글자열이다. 타입을 스키마로 알 수 없는 필드도 같다. `--project` · `--input` · `--help` 는 예약이다 — 툴 입력의 최상위 필드가 이 플래그 이름이 되면 카탈로그 시험이 실패한다. `--project` 는 `tool` 뒤 어디에나 둔다. 예약 플래그는 플래그 이름 자리에서만 가른다 — `--<플래그>` 바로 다음 자리는 값이라 `--message --project` · `--pattern -h` 의 뒤쪽은 값이다
- **비밀은 플래그로도 받지 않는다.** MCP 인자로 받지 않는 것(`db_connection_upsert` 의 비밀번호)은 CLI 에서도 UI 입력 요청 · env 참조뿐이다 — 플래그 값은 셸 기록과 프로세스 목록에 남는다
- **출력**: stdout 에는 결과 JSON 문서 하나만(`structuredContent` 가 있으면 그것, 없으면 텍스트 내용 — JSON 이 아닌 글이면 JSON 글자열로). 툴 오류 · 거절 이유 · 로그는 stderr. 사람이 읽기 좋은 표 출력은 두지 않는다 — `--help` 와 `list` 만 글이다. 로그 파일은 `$STATE/logs/cli.<날짜>.log`(03-architecture.md 「로그 · 진단」)
- **종료 코드**: 0 성공, 1 툴 오류 · 앱과의 연결 끊김, 2 사용법 오류(모르는 툴 · 플래그 포함. 툴 없이 `pitwall tool` 만 친 것도), 3 거절(노출되지 않은 툴 · 승인 거절 · 승인 시간 초과 · 헤드리스에서의 `write` 이상 · 앱이 있어야 하는 툴을 앱 없이 부름 · 툴의 관문이 `PERMISSION_DENIED` 로 거절 — 사람이 연 적 없는 DB 연결 · 미신뢰 · 프로덕션 등, 16-m5-plan M5-06), 128 + 신호 번호 — Ctrl+C 로 호출을 취소했다. 1 과 3 은 서버의 결과 표지로 가른다(「툴 레지스트리」 — `tools/call` 결과의 `_meta`, 노출 안 된 툴의 JSON-RPC 오류 `data`). 「앱이 있어야 하는 툴을 앱 없이」 는 CLI 가 가른다 — 실행 파일에 링크된 카탈로그에는 있고 헤드리스 Host 가 처리하지 않는 이름이면 부르지 않고 3 과 「Pitwall 앱을 실행한 뒤 다시」 다. 노출 설정이 끈 툴은 그대로 불러 서버가 거절하고 감사 행을 남긴다
- **취소**: Ctrl+C 면 그 호출을 MCP 취소 알림(`notifications/cancelled`)으로 거둔다 — 서버가 그 호출의 Host RPC 를 취소하는 「로컬 소켓과 헤드리스」 의 「호출 취소」 길이다. 알림을 보낸 뒤 130 으로 끝난다
- **승인**: `write` 이상은 MCP 와 같은 승인 요청이 그 프로젝트 창에 뜨고 CLI 는 답을 기다린다(시한은 「권한 모델」 절의 승인 타임아웃). 앱에 붙은 `write` 이상 호출의 답이 1초 안에 오지 않으면 stderr 에 「승인을 묻는다면 그 프로젝트 창에서 답하라」 한 줄을 쓴다(승인 경로는 16-m5-plan M5-12 — 그 전에는 승인이 곧바로 거절이라 이 줄이 나오지 않는다). 헤드리스에는 코어 툴만 있고 승인 경로가 없어 그 가운데 `read` 툴만 성공한다(「로컬 소켓과 헤드리스」). `auto_approve` 는 CLI 호출에도 같은 뜻이다
- **CLI 의 「이 세션 동안 허용」**: CLI 는 호출마다 세션을 새로 열어 세션 허용이 다음 호출에 닿지 않는다. 그래서 CLI 호출의 승인에서 그 자리는 **「이 프로젝트 창이 열려 있는 동안 CLI 허용」**(그 프로젝트 · 그 툴)이다 — 앱이 기억하고 창을 닫으면 사라진다. 이 허용은 CLI 로 온 호출 전부에 닿는다: 서버는 CLI 를 클라이언트가 적는 이름으로만 알므로, 같은 기기의 다른 에이전트 · 스크립트가 CLI 를 불러도 그동안은 묻지 않는다. `dangerous` 에는 없다(매번). 「이 연결 열기」 승인도 CLI 에서는 같은 범위다(창이 열려 있는 동안 · 그 연결). 06-roadmap.md 결정 D61
- **감사 로그**: 호출마다 MCP 와 같은 행이 남고 클라이언트는 `pitwall-cli <버전>` 이다. 「에이전트 활동」 패널에 함께 보인다
- **CLI 에 없는 것**: Resources · Prompts(「리소스 & 프롬프트」 절 — 같은 내용은 조회 툴로 읽는다), `tools/list_changed` 알림(호출마다 세션을 새로 열어 그때의 노출을 본다), Streamable HTTP
- 명령줄에서의 자리(`tool` 이 서브커맨드가 되는 규칙)는 04-features.md 「F2. 파일로 앱 실행」 절의 명령줄 규칙

## 4. 리소스 & 프롬프트 (MCP Resources/Prompts)

- Resources: `pitwall://project/{id}/config`, `pitwall://project/{id}/run/{name}/log`, `pitwall://project/{id}/db/{conn}/schema`
- Prompts: `setup_project` — "이 프로젝트의 실행 구성과 DB 연결을 감지해서 설정해줘" 워크플로 템플릿
- M5 에 없다 — 실행 구성이 생기는 M6 뒤에 정한다(16-m5-plan 결정 M5-D33)

## 5. 설계 메모

- 툴 입력 스키마는 코어 Rust 타입에서 `schemars`로 생성 → UI 폼과 MCP 검증이 동일
- 응답은 토큰 절약을 위해 기본 요약 + `detail` 옵션 (예: `run_logs`는 기본 마지막 200줄). 한 건의 상한과 목록 `limit` 의 기본은 「툴 레지스트리」 의 「응답 크기」
- 설정 변경은 결국 `.pitwall/*.toml` 파일 변경이므로, 에이전트가 파일을 직접 고쳐도 동일하게 반영됨 (파일 감시). MCP 툴은 **검증 · 승인 · 자동감지**라는 부가가치를 제공
