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

### 실행 구성
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
| `db_query` | SQL 실행. 기본 **읽기 전용 트랜잭션 + 행 제한** | read / dangerous(쓰기) |

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

## 4. 리소스 & 프롬프트 (MCP Resources/Prompts)

- Resources: `pitwall://project/{id}/config`, `pitwall://project/{id}/run/{name}/log`, `pitwall://project/{id}/db/{conn}/schema`
- Prompts: `setup_project` — "이 프로젝트의 실행 구성과 DB 연결을 감지해서 설정해줘" 워크플로 템플릿

## 5. 설계 메모

- 툴 입력 스키마는 코어 Rust 타입에서 `schemars`로 생성 → UI 폼과 MCP 검증이 동일
- 응답은 토큰 절약을 위해 기본 요약 + `detail` 옵션 (예: `run_logs`는 기본 마지막 200줄)
- 설정 변경은 결국 `.pitwall/*.toml` 파일 변경이므로, 에이전트가 파일을 직접 고쳐도 동일하게 반영됨 (파일 감시). MCP 툴은 **검증 · 승인 · 자동감지**라는 부가가치를 제공
