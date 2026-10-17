# 05. MCP 서버 설계

에이전트(Claude Code, Orca 내 에이전트 등)가 Pitwall을 통해 **프로젝트 구성 · 설정 · 조회**를 할 수 있게 한다.
코드 편집 자체는 에이전트가 파일시스템에서 직접 하므로, MCP는 "IDE만 아는 정보와 설정"에 집중한다.

## 1. 전송 방식

| 방식 | 사용 | 비고 |
|---|---|---|
| stdio | `pitwall mcp [--project <path>]` | 실행 중 앱에 로컬 소켓으로 붙는 얇은 프록시. 앱이 꺼져 있으면 헤드리스 host 기동 |
| Streamable HTTP | `http://127.0.0.1:<port>/mcp` | 앱 실행 중에만. Bearer 토큰 필수 |

- `--project` 미지정 시 cwd로 프로젝트 자동 해석 (F2와 같은 규칙)
- 앱 미실행 + 헤드리스 모드: 승인이 필요한 툴은 거부(또는 `--approve-mode` 정책에 따름)
- MCP 서버는 Host 별 연결 풀을 두고 툴 호출의 `projectId` 로 라우팅한다([03-architecture.md](03-architecture.md) 「RPC 설계」)

### 로컬 소켓과 헤드리스

구현: `crates/pw-mcp`(`socket` · `proxy` · `server`), `apps/desktop/src-tauri/src/mcp.rs`.

- **앱 쪽**: 주 인스턴스가 시작하면 로컬 소켓을 연다. 세션 · 툴 · 감사 로그는 앱 안에서 돈다. 네트워크 포트는 열지 않는다
- **`pitwall mcp`**: 그 소켓이 있으면 붙어서 stdio 줄을 그대로 잇는 프록시다. 소켓이 없거나 받지 않으면 **헤드리스** — 같은 프로세스에 `LocalHost` 를 만들어 stdio 로 직접 서비스한다. 앱을 띄우지 않는다
  - 헤드리스는 레지스트리를 **읽기만** 한다: 파일이 없으면 빈 목록, 손상 · 마이그레이션이 필요한 파일도 건드리지 않고 빈 목록이다(복구 · 마이그레이션은 레지스트리를 소유한 앱의 일). 승인 경로가 없으므로 `read` 툴만 성공한다
  - Windows 는 소켓이 아직 없어 언제나 헤드리스다
- **위치**: `$STATE/run/<해시>.sock`. 경로가 `sun_path` 한도(103바이트, macOS 104 · Linux 108 의 작은 쪽)를 넘으면 `<임시 폴더>/pitwall-<uid>/<해시>.sock`(macOS `$TMPDIR` 는 사용자 전용). 양쪽이 같은 규칙으로 고른다
- **그 사용자만 붙는다**: 폴더는 0700 · 이 사용자 소유 · 심링크 아님(서버는 만들거나 0700 으로 고치고, 다른 사용자 소유 · 심링크면 소켓을 열지 않는다. 클라이언트는 같은 검사를 통과한 폴더에만 붙는다), 소켓 파일 0600, 연결마다 양쪽이 상대 uid(`getpeereid` · `SO_PEERCRED`)를 본다. macOS 는 소켓 파일 권한으로 connect 를 막지 않으므로 폴더 권한이 실제 방어선이다
- **워크트리 격리**: 소켓 이름의 해시는 단일 인스턴스 키(debug: 빌드 폴더 + `$DATA`, release: 앱 identifier — `apps/desktop/src-tauri/src/instance.rs`)와 `$STATE` 로 만든다. 다른 워크트리 · 다른 `$DATA` 의 `pitwall mcp` 는 다른 이름을 계산해 이 앱에 붙지 않고 헤드리스로 돈다
- **이미 있는 소켓**: 붙어 보고 받으면 다른 앱이 쓰는 중이라 열지 않는다. 받지 않으면(앱이 죽고 남은 파일) 지우고 연다. 소켓이 아닌 파일 · 심링크는 지우지 않는다. 앱이 끝날 때 소켓 파일이 남을 수 있는데, 다음 시작이 이 규칙으로 바꾼다
- **핸드셰이크**: 연결 직후 클라이언트가 JSON 한 줄(`protocol: "pitwall-mcp/1"`, `project`, `cwd`), 서버가 한 줄(`ok`, `error`)로 답한다. 그 뒤는 MCP 바이트다. 프로젝트 해석(`project.resolve`)은 앱이 한다
- **끊김**: 앱이 도중에 닫히면 프록시는 응답을 기다리던 요청마다 JSON-RPC 오류(`-32001`)를 stdout 에 쓰고 stderr 에 이유를 남긴 뒤 **종료 코드 1** 로 끝난다. 클라이언트가 stdin 을 닫으면 소켓의 쓰기 쪽을 닫아 앱 쪽 세션을 끝내고(남은 응답은 5초까지 옮긴다) 0 으로 끝난다. 앱 쪽 세션이 끝나면 그 세션의 Host 연결 · 설정 감시 · 버스 구독도 걷힌다
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
| `settings_get` / `settings_set` | 프로젝트 설정 키 조회/변경 | read / write |

### 실행 구성 (M6 — 실행 기능과 함께 제공)
| 툴 | 설명 | 권한 |
|---|---|---|
| `run_config_list` | 실행 구성 + 감지된 태스크 | read |
| `run_config_upsert` | 실행 구성 생성/수정 (스키마 검증) | write |
| `run_config_delete` | 삭제 | write |
| `run_start` / `run_stop` / `run_restart` | 구성 실행 제어 | exec |
| `run_status` | 실행 중 프로세스, 포트, 종료 코드 | read |
| `run_logs` | 로그 조회 (tail, since, grep 필터) | read |

### 디버그
| 툴 | 설명 | 권한 |
|---|---|---|
| `breakpoint_list` / `breakpoint_set` | 브레이크포인트 조회/설정 | read / write |
| `debug_state` | 현재 정지 위치, 스택, 지역 변수 스냅샷 | read |

### DB
| 툴 | 설명 | 권한 |
|---|---|---|
| `db_connection_list` | 연결 목록 (비밀 제외) | read |
| `db_connection_upsert` | 연결 추가/수정 — 비밀번호는 툴 인자로 받지 않고 UI 입력 요청 또는 env 참조 | write |
| `db_connection_test` | 연결 테스트 | read |
| `db_schema` | 스키마/테이블/컬럼/인덱스 조회 | read |
| `db_query` | SQL 실행. 기본 **읽기 전용 트랜잭션 + 행 제한**. SQL 위험도 분류(04 F6-9) 결과로 등급 결정 | read / dangerous(쓰기 · DDL) |

### Git
| 툴 | 설명 | 권한 |
|---|---|---|
| `git_status` / `git_log` / `git_blame` / `git_diff` | 조회 | read |
| `git_branch_list` | 브랜치 | read |
| (쓰기 작업은 의도적으로 제외) | 에이전트는 git CLI를 직접 쓰면 되므로 중복 제공하지 않음 | |

### 코드 인텔리전스
| 툴 | 설명 | 권한 |
|---|---|---|
| `diagnostics` | LSP 진단 (파일/프로젝트 단위, 심각도 필터) | read |
| `symbol_search` | 워크스페이스 심볼 검색 | read |
| `find_references` / `goto_definition` | 위치 기반 조회 | read |

### 외부 도구
| 툴 | 설명 | 권한 |
|---|---|---|
| `open_with_list` / `open_with_upsert` | 외부 도구 템플릿 관리 | read / write |

`*` UI에 영향을 주지만 비파괴 → 승인 불필요

### 툴 레지스트리 (RPC → 툴)

툴은 손으로 등록하지 않고 RPC 선언(`rpc_method!`)에서 만든다. 구현: `crates/pw-mcp/src/catalog.rs`.

- **노출은 선언이 고른다**: `rpc_method!` 의 선택 필드 `mcp: true` 를 단 메서드만 툴이 된다. 생략하면 꺼짐 — **새 RPC 는 말없이 툴이 되지 않는다**(UI 전용 메서드 `host.demoTask` · `project.markOpened` 등). `crates/pw-mcp/tests/session.rs` 의 `only_methods_marked_mcp_become_tools` 가 노출 목록을 고정하므로, 툴을 늘리려면 선언과 그 목록을 함께 고친다. 스트림 메서드에는 달 수 없다(컴파일 오류)
- **role 은 노출을 정하지 않는다**: role 은 「어느 기기가 부를 수 있나」다. MCP 세션은 Host 에 **owner** 로 붙고(로컬 소켓 · stdio 로만 오는 이 사용자의 프로세스다), 부를 수 있는지는 「툴 노출 설정」 → 「권한 모델」 절이 정한다
- **이름**: RPC 이름의 `.` → `_`, camelCase → snake_case(`project.list` → `project_list`, `project.markOpened` → `project_mark_opened`). `[a-z0-9_]` 64자 이내, 겹치면 시작 시 실패
- **설명 · 등급**: 선언의 문서 주석과 `tier`. `read` 툴에는 `readOnlyHint` 를 단다
- **입력 스키마**: 입력 타입의 schemars 스키마를 루트로 펼친다. struct 만 툴이 된다(MCP `inputSchema` 는 object). `()` 입력은 빈 object 로 내보내고 RPC 에는 `null` 을 보낸다. enum 루트는 툴로 만들 수 없다
- **출력**: 결과 스키마의 루트가 object 면 `outputSchema` 와 `structuredContent`, 아니면(배열 등) JSON 텍스트만
- **프로젝트 지정**: 입력에 `projectId` 가 있으면 프로젝트 툴이다. 세션에 프로젝트가 있으면(`--project` · cwd) `projectId` 를 필수에서 빼고 빠지면 서버가 채운다
- **지금 툴**: `project_list`(← `project.list`), `project_info`(← `project.info`: 프로젝트 · 루트 폴더 유무 · 빌드 파일 · 빌드 파일에서 짐작한 언어 · 설정 폴더와 그 안의 설정 파일 id. 설정 내용은 읽지 않는다 — 읽으면 구버전 파일의 마이그레이션이 따라와 조회 툴이 저장소를 고친다)
- MCP 메시지(툴 설명 · 오류)는 번역하지 않는다

## 3. 권한 모델

| 등급 | 예 | 기본 정책 |
|---|---|---|
| `read` | 조회 전부 | 자동 허용 |
| `write` | 설정 파일 변경 | **UI 승인** (diff 미리보기). "이 세션 동안 허용" 옵션 |
| `exec` | 프로세스 실행 | 신뢰된 프로젝트 + 이미 존재하는 구성: 자동 / 방금 생성·수정된 구성: 승인 |
| `dangerous` | DB 쓰기 쿼리, 프로덕션 태그 연결 접근 | 매번 승인, 프로덕션은 기본 차단 |

- 정책은 전역/프로젝트 설정에서 조정 가능 (`[mcp.policy]`)
- **지금 구현**(`crates/pw-mcp/src/policy.rs`): `read` 는 허용, `write` 이상은 승인 화면이 아직 없어 **거절**한다(이유를 툴 오류로 돌려주고 감사 로그에 `denied`). `[mcp.policy]` · `auto_approve` 는 아직 읽지 않는다
- 모든 MCP 호출은 감사 로그에 기록 → UI "에이전트 활동" 패널

### 감사 로그

- **한 파일 `$STATE/mcp/audit.db`(SQLite)** 에 `project_id` 열을 둔다. 프로젝트를 정하지 않는 호출(`project_list`)은 `project_id` 가 `NULL` 이다. 프로젝트마다 파일을 두지 않는 이유: 어느 프로젝트에도 속하지 않는 호출이 있고, `$STATE` 가 감사 기록의 자리이며([03-architecture.md](03-architecture.md) 「데이터 저장」 표), 보관 기간 정리가 한 곳에서 끝난다. 「에이전트 활동」 패널은 `project_id` 로 거른다
- 한 행 = `tools/call` 하나: 시각 · 세션 id · 클라이언트(`clientInfo` 이름 버전) · 프로젝트 · 툴 · 인자 요약 · 결과(`ok` · `error` · `denied`) · 오류나 거절 이유 · 결과 크기(바이트) · 소요(ms). **결과 내용은 남기지 않는다.** 노출되지 않은 툴 호출도 `denied` 로 남는다
- **인자 요약**: 키를 단어로 갈라(camelCase · `_` · `-`) `password` · `secret` · `token` · `key` · `auth` · `credential` · `cookie` · `private` 등이 있으면 값을 `[redacted]` 로 바꾼다. 글자열은 80자, 배열은 8개, 깊이 3, 전체 1024바이트까지. 키 이름으로 못 거르는 비밀이 남을 수 있으므로 비밀을 툴 인자로 받지 않는다는 규칙(`db_connection_upsert`)이 첫 방어선이다. 목록은 `pw_mcp::audit::SECRET_WORDS`
- 여러 프로세스(앱, 헤드리스 `pitwall mcp` 여럿)가 같은 파일에 쓴다(`BEGIN IMMEDIATE`, 2초 대기). 기록이 실패해도 툴 호출은 실패시키지 않고 경고 로그를 남긴다 — 쓰기 등급 툴을 열 때 다시 정한다. 보관 기간 · 지우기는 감사 로그 UI 와 함께 정한다
- **승인 요청 라우팅**: 해당 프로젝트 창이 열린 Desktop에 다이얼로그 + OS 알림. 원격 Relay에서 온 요청도 연결된 Desktop으로 전달. 타임아웃(기본 5분) 시 거절. (모바일 승인은 모바일 보류에 따라 제외)
- 원격 머신에서 도는 에이전트도 해당 Relay의 MCP 엔드포인트를 사용 → 같은 승인 흐름
- 확장(WASM 플러그인)이 추가한 MCP 툴은 `<ext-id>.<tool>` 이름으로 노출, 권한 등급은 확장 매니페스트에 선언

## 3-1. 툴 노출 설정 (툴별 on/off)

JetBrains IDE 내장 MCP 서버의 "Exposed Tools" 방식을 따른다. 권한 등급(3절)과 별개로 **툴 자체를 에이전트에게 보이지 않게** 할 수 있다.

| 항목 | 설계 |
|---|---|
| 단위 | 툴 단위 on/off (꺼진 툴은 `tools/list`에 나오지 않음 → 에이전트 컨텍스트 토큰도 절약) |
| 그룹 | 모듈 그룹(project / run / debug / db / git / code / open_with / 확장별) 일괄 on/off |
| 프리셋 | `read-only`(조회 툴만) · `standard`(기본: 조회 + 설정) · `full`(실행 · DB 쓰기 포함) |
| 범위 | 전역 기본값 → 프로젝트별 덮어쓰기 (`.pitwall/project.toml` 또는 `.local.toml`, 개인 전용 프로젝트는 `$CONFIG/projects/<slug>/project.toml`) |
| 자동 승인 | 등급별 "확인 없이 실행" 토글 (JetBrains의 brave mode에 해당). `dangerous` 등급은 자동 승인 불가 |
| 변경 반영 | 설정 변경 시 MCP `notifications/tools/list_changed` 전송 → 연결된 에이전트가 즉시 갱신 |
| 확장 툴 | 확장이 추가한 툴은 **기본 꺼짐**, 사용자가 켜야 노출 |
| UI | 설정 > MCP: 툴 목록(이름 · 설명 · 등급 · 최근 호출 수), 검색, 그룹 토글, 입력 스키마 미리보기 |

지금 구현(`crates/pw-mcp/src/exposure.rs`):
- 읽는 키는 `preset` 과 `[mcp.tools]` 다. 전역 `[mcp]` 위에 프로젝트 `[mcp]` 를 설정 계층의 병합 규칙으로 덮는다(`[mcp.tools]` 는 키별로 합쳐진다). 내장 기본값은 `preset = "standard"`
- 프리셋은 등급으로 고른다: `read-only` = `read`, `standard` = `read` · `write`, `full` = 전부
- 판정 순서: 툴 이름과 정확히 같은 키 → 맞는 glob(`*` 는 아무 글자열) 중 가장 긴 것 → 프리셋. 모듈 그룹 on/off 는 지금 glob 으로 쓴다(`"project_*" = false`)
- 잘못된 값은 버리고 경고 로그를 남긴다. 모르는 프리셋은 가장 좁은 `read-only` 로 본다
- 변경 반영: 전역 · 세션 프로젝트의 설정 파일을 감시해 **보이는 툴 집합이 바뀌면** `notifications/tools/list_changed` 를 보낸다. 2025-11-25 까지의 클라이언트는 initialize 뒤 세션 알림으로, 2026-07-28 클라이언트는 `subscriptions/listen` 구독으로 받는다
- `auto_approve` · 확장 툴 기본 꺼짐 · 설정 > MCP 화면은 아직 없다

```toml
# ~/.config/pitwall/settings.toml (전역) — 프로젝트에서 같은 키로 덮어쓰기
[mcp]
preset = "standard"
auto_approve = ["read", "write"]      # "exec" 추가 가능, "dangerous"는 불가

[mcp.tools]
db_query = true
run_start = false                      # 개별 툴 끄기
"dev.pitwall.ext-kafka.*" = true       # 확장 툴 켜기 (glob)
```

### 클라이언트 자동 등록 (F10-5)
- 전역 등록: Claude Code(`~/.claude.json`), Cursor, Codex 등 감지된 클라이언트 설정에 `pitwall mcp` 항목 추가
- 프로젝트 등록: 프로젝트 루트의 `.mcp.json` 등에 추가 → 해당 프로젝트에서만 Pitwall 툴 노출
- 등록 전 변경 diff를 보여주고 승인 후 기록, 해제(제거)도 같은 화면에서

## 4. 리소스 & 프롬프트 (MCP Resources/Prompts)

- Resources: `pitwall://project/{id}/config`, `pitwall://project/{id}/run/{name}/log`, `pitwall://project/{id}/db/{conn}/schema`
- Prompts: `setup_project` — "이 프로젝트의 실행 구성과 DB 연결을 감지해서 설정해줘" 워크플로 템플릿

## 5. 설계 메모

- 툴 입력 스키마는 코어 Rust 타입에서 `schemars`로 생성 → UI 폼과 MCP 검증이 동일
- 응답은 토큰 절약을 위해 기본 요약 + `detail` 옵션 (예: `run_logs`는 기본 마지막 200줄)
- 설정 변경은 결국 `.pitwall/*.toml` 파일 변경이므로, 에이전트가 파일을 직접 고쳐도 동일하게 반영됨 (파일 감시). MCP 툴은 **검증 · 승인 · 자동감지**라는 부가가치를 제공
