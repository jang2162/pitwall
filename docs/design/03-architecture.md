# 03. 아키텍처

## 1. 전체 구조

```
┌──────────────────────── Desktop (Tauri) ─────────────────────────┐
│  React UI (프로젝트 창 N개)                                         │
│     │  JSON-RPC (Tauri IPC)  + 이벤트 스트림                         │
│  ┌──▼──────────────────────────────────────────────────────────┐  │
│  │ Host Router  — project_id → Host 연결로 라우팅                │  │
│  └──┬───────────────────────────────┬──────────────────────────┘  │
│     │ in-process                    │ SSH stdio (JSON-RPC)        │
│  ┌──▼──────────────┐                │                             │
│  │ Local Host      │                │                             │
│  │ (pw-core 등)     │                │                             │
│  └──▲──────────────┘                │                             │
│     │                               │                             │
│  MCP Server (rmcp)  ── stdio / Streamable HTTP(127.0.0.1) ◄── 에이전트 │
└─────────────────────────────────────┼─────────────────────────────┘
                                      ▼
                        ┌────── 원격 머신 ───────┐
                        │ pitwall-host (동일 코어) │
                        │ fs / git / lsp / dap /  │
                        │ run / db(옵션)           │
                        └────────────────────────┘
```

**핵심: "Host" = 한 머신에서 프로젝트를 다루는 헤드리스 서비스.**
- 로컬 프로젝트 → 데스크톱 프로세스 안의 Local Host가 처리
- 원격 프로젝트 → 원격에 배포된 `pitwall-host`가 처리, 데스크톱은 같은 RPC를 SSH 위로 전달
- UI와 MCP는 Host가 로컬인지 원격인지 몰라도 된다

## 2. 코어 모듈 (Host 내부)

| 모듈 | 책임 | 주요 이벤트 |
|---|---|---|
| `workspace` | 프로젝트 열기/닫기, 설정 로드, 모듈 수명주기 | `project.opened`, `config.changed` |
| `fs` | 파일 트리, 읽기/쓰기, 감시, 파일/텍스트 검색 | `fs.changed` |
| `git` | status/log/graph/blame/diff, 쓰기 작업, 저장소 감시 | `git.refsChanged`, `git.statusChanged` |
| `index` | tree-sitter 파싱, 심볼 인덱스 | `index.progress` |
| `lsp` | 언어 서버 수명주기, 요청 프록시, 진단 집계 | `lsp.diagnostics`, `lsp.status` |
| `run` | 실행 구성, 태스크 감지, 프로세스/PTY, 로그 버퍼 | `run.output`, `run.exited` |
| `dap` | 디버그 세션, 브레이크포인트, 스택/변수 | `dap.stopped`, `dap.output` |
| `db` | 연결 풀, 쿼리 실행(스트리밍), 스키마 캐시 | `db.queryProgress` |
| `secrets` | 키체인 접근 (로컬에서만; 원격 host에는 필요 시 전달) | |

- 모든 모듈은 `ProjectContext`(루트 경로, 설정, 이벤트 버스, 취소 토큰)를 받는다
- 장기 작업은 `Task` 추상(진행률, 취소)으로 통일 → UI 상태바 / MCP 둘 다 표시 가능

## 3. 프로세스 · 창 모델

- **한 프로세스, 프로젝트당 한 창** (IntelliJ와 동일한 감각)
- 프로젝트 창을 닫으면 해당 프로젝트의 LSP/DAP/실행 프로세스 정리 (실행 중 프로세스는 확인 후)
- 단일 인스턴스: 두 번째 실행(`pitwall path`)은 기존 인스턴스에 인자를 전달하고 종료
- 웰컴 창: 최근 프로젝트, 원격 호스트, 새 프로젝트

## 4. RPC 설계

- JSON-RPC 2.0, 메서드명 `<module>.<action>` (예: `git.log`, `db.query.execute`)
- 요청은 항상 `projectId` 포함 → Host Router가 라우팅
- 대용량 응답(로그, 쿼리 결과, git log)은 **스트림**: `streamId`를 반환하고 `stream.chunk` 알림으로 전달, 백프레셔는 credit 방식
- 이벤트 구독: `events.subscribe({ projectId, topics })`
- 타입 정의 단일 소스: Rust 구조체 → specta(TS) / schemars(JSON Schema, MCP)

## 5. 데이터 저장

| 범위 | 위치 | 형식 | 내용 |
|---|---|---|---|
| 앱 전역 | `~/.pitwall/` (OS별 app data dir) | `settings.toml`, `state.db`(SQLite) | 전역 설정, 프로젝트 레지스트리, 최근 목록, 원격 호스트, 외부 도구 목록 |
| 프로젝트 공유 설정 | `<project>/.pitwall/project.toml`, `run/*.toml`, `db/*.toml` | TOML | 커밋 가능. 실행 구성, DB 연결(비밀 제외), 외부 도구 오버라이드 |
| 프로젝트 로컬 상태 | `<appdata>/projects/<id>/` | SQLite | 열린 탭, 레이아웃, 브레이크포인트, 쿼리 히스토리, 심볼 인덱스 캐시 |
| 비밀 | OS 키체인 | | DB 비밀번호, SSH 패스프레이즈, 토큰. 설정 파일에는 `secret_ref`만 |

- `.pitwall/`는 기본 **커밋 가능** 형태로 설계. 개인용 덮어쓰기는 `.pitwall/*.local.toml` (자동으로 `.gitignore` 추천)
- 기존 설정 임포트 (편의): `.idea/runConfigurations/*.xml`, `.idea/dataSources.xml`, `.vscode/launch.json`, `.vscode/tasks.json`

### 프로젝트 설정 예시

```toml
# .pitwall/project.toml
name = "billing-api"
[languages]
enabled = ["kotlin", "sql"]

[open_with]          # 프로젝트별 외부 도구 오버라이드
default = "orca"
```

```toml
# .pitwall/run/dev-server.toml
name = "Dev Server"
type = "shell"               # shell | npm | gradle | cargo | docker-compose | ...
command = "./gradlew bootRun"
cwd = "."
env = { SPRING_PROFILES_ACTIVE = "local" }
env_file = ".env.local"
[debug]
adapter = "java"
mode = "attach"
port = 5005
```

```toml
# .pitwall/db/local-pg.toml
name = "local postgres"
driver = "postgres"
host = "localhost"
port = 5432
database = "billing"
user = "app"
password = { secret_ref = "keychain" }
read_only = false
[ssh_tunnel]            # 선택
host_ref = "bastion"
```

## 6. 원격 연결 구조

1. 사용자가 SSH 호스트 등록 (`~/.ssh/config` 호스트 자동 임포트)
2. 연결 시 원격의 OS/아키텍처 감지 → 버전이 맞는 `pitwall-host` 바이너리를 업로드(또는 원격에서 다운로드)하여 `~/.pitwall-server/<version>/`에 설치
3. `ssh host pitwall-host --stdio` 로 기동, JSON-RPC 연결
4. 포트 포워딩: 원격에서 실행한 웹 서버 · 디버그 포트를 로컬로 포워딩 (DAP attach용)
5. 재연결: 연결이 끊겨도 원격 host는 유예 시간(기본 10분) 동안 프로세스를 유지 → 재접속 시 세션 복구
6. DB 연결은 두 가지 모드: (a) 로컬에서 SSH 터널, (b) 원격 host에서 직접 연결 — 프로젝트가 원격이면 (b) 기본

## 7. 보안

- MCP HTTP 서버는 `127.0.0.1`에만 바인드 + 기동 시 생성되는 Bearer 토큰 필수
- 위험 작업(파괴적 git, DB 쓰기, 실행 구성의 임의 커맨드 실행)은 MCP 경유 시 UI 승인 필요 (→ 05-mcp.md)
- 원격 host는 SSH 채널 stdio로만 통신 (원격 포트 개방 없음)
- 프로젝트 설정의 실행 커맨드는 "신뢰된 프로젝트"에서만 실행 (VS Code Workspace Trust와 유사)
