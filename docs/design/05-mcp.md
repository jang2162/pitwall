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

## 3. 권한 모델

| 등급 | 예 | 기본 정책 |
|---|---|---|
| `read` | 조회 전부 | 자동 허용 |
| `write` | 설정 파일 변경 | **UI 승인** (diff 미리보기). "이 세션 동안 허용" 옵션 |
| `exec` | 프로세스 실행 | 신뢰된 프로젝트 + 이미 존재하는 구성: 자동 / 방금 생성·수정된 구성: 승인 |
| `dangerous` | DB 쓰기 쿼리, 프로덕션 태그 연결 접근 | 매번 승인, 프로덕션은 기본 차단 |

- 정책은 전역/프로젝트 설정에서 조정 가능 (`[mcp.policy]`)
- 모든 MCP 호출은 감사 로그(프로젝트 로컬 SQLite)에 기록 → UI "에이전트 활동" 패널
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
| 범위 | 전역 기본값 → 프로젝트별 덮어쓰기 (`.pitwall/project.toml` 또는 `.local.toml`) |
| 자동 승인 | 등급별 "확인 없이 실행" 토글 (JetBrains의 brave mode에 해당). `dangerous` 등급은 자동 승인 불가 |
| 변경 반영 | 설정 변경 시 MCP `notifications/tools/list_changed` 전송 → 연결된 에이전트가 즉시 갱신 |
| 확장 툴 | 확장이 추가한 툴은 **기본 꺼짐**, 사용자가 켜야 노출 |
| UI | 설정 > MCP: 툴 목록(이름 · 설명 · 등급 · 최근 호출 수), 검색, 그룹 토글, 입력 스키마 미리보기 |

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
