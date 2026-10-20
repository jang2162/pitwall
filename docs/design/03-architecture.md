# 03. 아키텍처

## 1. 전체 구조

```
┌──────────────────────── Desktop (Tauri) ─────────────────────────┐
│  React UI (프로젝트 창 N개)                                         │
│     │  JSON-RPC (Tauri IPC)  + 이벤트 스트림                         │
│  ┌──▼──────────────────────────────────────────────────────────┐  │
│  │ Host Router  — project_id → Host 연결로 라우팅                │  │
│  └──┬───────────────────────────────┬──────────────────────────┘  │
│     │ in-process                    │ pw-link (QUIC 직접 / SSH)     │
│  ┌──▼──────────────┐                │                             │
│  │ Local Host      │                │                             │
│  │ (pw-core 등)     │                │                             │
│  └──▲──────────────┘                │                             │
│     │                               │                             │
│  MCP Server (rmcp)  ── stdio / Streamable HTTP(127.0.0.1) ◄── 에이전트 │
└─────────────────────────────────────┼─────────────────────────────┘
                                      ▼
                        ┌────── 원격 머신 ───────┐
                        │ pitwall-relay (동일 코어)│
                        │ fs / git / lsp / dap /  │
                        │ run / db(옵션)           │
                        └────────────────────────┘
```

**핵심: "Host" = 한 머신에서 프로젝트를 다루는 헤드리스 서비스.**
- 로컬 프로젝트 → 데스크톱 프로세스 안의 Local Host가 처리
- 원격 프로젝트 → 원격에 설치된 `pitwall-relay`가 처리, 데스크톱은 같은 RPC를 `pw-link` 위로 전달
- (보류) 모바일을 붙일 경우에도 같은 RPC를 `pw-link`로 사용하도록 role 메타데이터를 유지
- 확장(`pw-ext`)은 Host 안에서 로드 → 원격 Relay에서도 같은 확장이 동작
- UI와 MCP는 Host가 로컬인지 원격인지 몰라도 된다

## 2. 코어 모듈 (Host 내부)

| 모듈 | 책임 | 주요 이벤트 |
|---|---|---|
| `workspace` | 프로젝트 열기/닫기, 설정 로드, 모듈 수명주기 | `project.opened`, `config.changed` |
| `fs` | 파일 트리, 읽기/쓰기, 감시, 파일/텍스트 검색 | `fs.changed` |
| `git` | status/log/graph/blame/diff, 쓰기 작업, 저장소 감시 | `git.refsChanged`, `git.statusChanged` |
| `index` | tree-sitter 파싱, 심볼 인덱스 | `index.progress` |
| `lsp` | 언어 서버 수명주기, 요청 프록시, 진단 집계 | `lsp.diagnostics`, `lsp.status` |
| `runtime` | 런타임 환경 해석 (mise · direnv · asdf · 로그인 셸 · 수동), 디렉터리별 env 캐시 — §5-1 | `runtime.changed` |
| `run` | 실행 구성, 태스크 감지, 프로세스/PTY, 로그 버퍼 | `run.output`, `run.exited` |
| `dap` | 디버그 세션, 브레이크포인트, 스택/변수 | `dap.stopped`, `dap.output` |
| `db` | 연결 풀, 쿼리 실행(스트리밍), 스키마 캐시 | `db.queryProgress` |
| `secrets` | 키체인 접근 (로컬에서만; 원격 Relay에는 필요 시 전달) | |

- 모든 모듈은 `ProjectContext`(루트 경로, 설정, 이벤트 버스, 취소 토큰)를 받는다
- 파일 감시(M1-06, 12-m1-plan.md 결정 M1-D7): 프로젝트 창이 `fs.watch { projectId, worktree? }` 스트림을 여는 동안 Host 가 창의 루트(활성 워크트리가 있으면 그 안의 프로젝트 루트)를 감시한다. 스트림이 닫히면(창이 닫히거나 연결이 끊김) 놓고, 활성 워크트리가 바뀌면 창이 닫고 다시 연다. 같은 루트를 보는 창이 여럿이면 OS 감시는 하나다. 바뀐 경로는 프로젝트 범위 토픽 `fs.changed { root, worktree, paths, kinds, rescan }` 로 나간다 — 디바운스는 고정 창(쉬던 중 첫 이벤트가 75 ms 창을 연다)이라 계속 쓰는 동안에도 75 ms 마다 나간다. `kinds[i]` 는 창이 닫힐 때 디스크를 보고 정해 `removed` 는 확실하고 `created` · `modified` 는 힌트다. `rescan` 이면(OS 큐 넘침 · 감시 오류 · 창 하나에 4096 경로 초과) `paths` 가 비고 받는 쪽이 루트를 다시 읽는다. `.git/` 안은 거르고, 심링크는 따르지 않는다(루트 밖을 가리키는 링크 너머의 변경은 오지 않는다). 에이전트식 쓰기(임시 파일 → rename)의 대상은 변경 하나로 보이고, 임시 파일 경로는 OS 의 「생성」 이 정확한 곳(Linux · Windows)에서만 뺀다 — macOS FSEvents 는 같은 경로의 최근 기록을 플래그로 합쳐 주어, 그것을 믿고 빼면 방금 만든 파일의 진짜 삭제를 잃는다(그래서 macOS 에서는 임시 파일 경로가 `removed` 로 함께 온다). 스트림의 항목은 감시 방식(`recursive` · `folders` · `polling`)이다
- Host → 창 알림: 버스 토픽 `notification.show { level, message, args?, body? }`(Host 또는 프로젝트 범위). `message` 는 Fluent 메시지 id 라 창이 자기 언어로 제목을 만들고, `body` 는 원문이다. 프로젝트 창이 Host 범위와 자기 프로젝트 범위를 구독해 토스트 · 알림 기록으로 띄운다(10-ui.md 「빈 상태 · 오류 상태」). 버스는 지난 이벤트를 남기지 않으므로 창이 없을 때 낸 알림은 사라진다 — 놓치면 안 되는 상태는 출처를 읽는 길(상태바 등)을 따로 둔다
- 장기 작업은 `Task` 추상(진행률, 취소)으로 통일 → UI 상태바 / MCP 둘 다 표시 가능. 진행 · 종료는 버스 토픽 `task.progress` · `task.finished`(범위는 띄운 곳 — Host 또는 프로젝트)로 나가고, 구독 전에 시작한 작업은 `task.list { projectId? }`(Host 범위 + 그 프로젝트) 로 읽으며, 띄운 쪽이 아니어도 `task.cancel { taskId }`(tier write — Owner 만)로 멈춘다. 둘의 범위는 연결의 프로젝트가 아니라 params 다: `task.cancel` 은 같은 Host 의 모든 작업을 멈출 수 있고 `task.list` 는 params 의 프로젝트를 읽는다(이벤트 구독과 같은 규칙 — 원격 Relay 에 연결별 프로젝트 제한이 생기면 이 둘도 그 검사에 넣는다). 프로젝트 창 상태바는 이 셋으로 작업을 보이고 실패는 오류 알림으로 낸다(10-ui.md 「빈 상태 · 오류 상태」)

## 3. 프로세스 · 창 모델

- **한 프로세스, 프로젝트당 한 창** (IntelliJ와 동일한 감각)
- 프로젝트 창을 닫으면 해당 프로젝트의 LSP/DAP/실행 프로세스 정리 (실행 중 프로세스는 확인 후)
- 단일 인스턴스: 두 번째 실행(`pitwall path`)은 기존 인스턴스에 인자와 cwd 를 전달하고 종료. 첫 실행도 같은 열기 경로를 지난다(04 F2 「명령줄 규칙」). 키는 앱 identifier 하나다 — release 는 그대로 하나, **debug 빌드는 identifier 에 「빌드한 크레이트 폴더 + `$DATA`」의 해시를 붙여** 워크트리마다 · 데이터 폴더마다 갈린다(여러 워크트리의 dev 앱이 서로에게 전달되지 않게). release 빌드도 `PITWALL_DATA_DIR` 로 기본이 아닌 `$DATA` 를 주면 identifier 에 그 `$DATA` 의 해시를 붙인다 — 기본 `$DATA` 의 정식 앱은 키 하나 그대로이고, 임시 데이터로 띄운 release 빌드(성능 측정 · 시험)는 정식 앱과 인자를 주고받지 않는다. 그래서 Windows · Linux 에서는 키가 갈린 빌드마다 웹뷰 데이터 폴더(localStorage)도 따로다. macOS WKWebView 는 키가 아니라 번들 identifier 로 저장소를 정하므로, 기본이 아닌 `$DATA` 로 뜬 앱(debug 포함)은 그 `$DATA` 로 정한 별도 웹뷰 저장소(`data_store_identifier`, macOS 14+)를 쓴다
- 웰컴 창: 최근 프로젝트, 원격 호스트, 새 프로젝트

## 4. RPC 설계

- JSON-RPC 2.0, 메서드명 `<module>.<action>` (예: `git.log`, `db.query.execute`)
- Host Router 는 연결 단위로 라우팅한다. 클라이언트는 `projectId`(또는 프로젝트가 없는 창은 `hostId`)로 Host 연결을 열고, 그 연결의 요청 · 스트림 · 구독 id 는 그 Host 세션 안에서만 유효하다. 기능 메서드의 params 에는 여전히 `projectId` 를 넣어 Host 안에서 `ProjectContext` 를 고른다. 여러 Host 의 프로젝트를 한 세션에서 다루는 클라이언트(MCP 서버)는 Host 별 연결을 하나씩 두고 요청마다 `projectId` 로 연결을 고른다. 프로젝트를 다른 Host 로 다시 묶으면 그 프로젝트의 기존 연결은 닫고 새로 연다.
- 대용량 응답(로그, 쿼리 결과, git log)은 **스트림**: `streamId`를 반환하고 `stream.chunk` 알림으로 전달, 백프레셔는 credit 방식
- 이벤트 구독: `events.subscribe({ projectId, topics })`. `projectId` 를 빼면 Host 범위(프로젝트에 속하지 않는 이벤트 — 전역 설정 변경 등)를 받는다. 프로젝트 없는 창은 이것으로 구독한다
- 타입 정의 단일 소스: Rust 구조체 → specta(TS) / schemars(JSON Schema, MCP)
- 프로젝트 전용 오류 코드는 `-32000..=-32099` 에서 고르고 `pw_rpc::code` 한 곳에 둔다(생성기가 TS 상수로 내보낸다): `PERMISSION_DENIED`(-32001, role) · `CANCELLED`(-32002) · `STORAGE_UNAVAILABLE`(-32003, §5-0) · `CONFLICT`(-32004, 기대한 디스크 상태와 달라 쓰지 않음 — `fs.write` 의 기대 `version`, `data` 에 지금 상태) · `NOT_FOUND`(-32005, 대상 파일 · 폴더가 없음)

## 5. 데이터 저장

### 5-0. 파일 위치 — XDG 규칙 (D27)

macOS · Linux 모두 **XDG Base Directory** 규칙을 따른다 (mise · gh · Zed 등 개발 도구 관례, `~/Library/Application Support` 대신). Windows는 표준 폴더에 대응시킨다.

| 이름 | 용도 | macOS / Linux 기본값 | Windows |
|---|---|---|---|
| `$CONFIG` | **사람이 편집하는 설정만** | `$XDG_CONFIG_HOME/pitwall` → `~/.config/pitwall` | `%APPDATA%\pitwall\config` |
| `$DATA` | 앱이 관리하는 영구 데이터 | `$XDG_DATA_HOME/pitwall` → `~/.local/share/pitwall` | `%LOCALAPPDATA%\pitwall\data` |
| `$STATE` | 로그 · 백업 · 감사 기록 | `$XDG_STATE_HOME/pitwall` → `~/.local/state/pitwall` | `%LOCALAPPDATA%\pitwall\state` |
| `$CACHE` | 지워도 되는 캐시 | `$XDG_CACHE_HOME/pitwall` → `~/.cache/pitwall` | `%LOCALAPPDATA%\pitwall\cache` |

- 환경 변수로 개별 덮어쓰기: `PITWALL_CONFIG_DIR`, `PITWALL_DATA_DIR`, `PITWALL_STATE_DIR`, `PITWALL_CACHE_DIR` (테스트 · 포터블 실행용)
  - 우선순위: `PITWALL_*_DIR`(그 폴더 자체) → `XDG_*_HOME`/`pitwall` → 홈 아래 XDG 기본값. 빈 값은 없는 것으로 본다
  - `PITWALL_*_DIR` 가 상대 경로면 시작 오류(실행 위치마다 다른 폴더가 되므로). 상대 경로인 `XDG_*_HOME` 은 XDG 명세대로 무시
  - Windows 는 `XDG_*` 를 보지 않는다. 구현: `pw_core::config::paths`
- **`$CONFIG` 규칙**: 텍스트(TOML)만, 비밀 · 기기 고유 상태 · 캐시 없음 → **chezmoi · yadm 등 dotfiles 도구로 그대로 동기화 가능** (기기 간 설정 동기화의 기본 방법)
  - 설정 파일이 심링크면 따라가서 읽고, 마이그레이션은 링크가 아니라 **대상 파일**을 고친다(권한 유지). 대상이 없는 심링크는 건너뛰고 알림(`brokenLink`)으로 남긴다
- 기기 고유 값이 필요하면 `settings.local.toml`(같은 폴더, dotfiles에서 제외 권장)으로 분리
- 원격 Relay도 같은 규칙: `~/.config/pitwall/relay.toml`, 바이너리 `~/.local/share/pitwall/relay/`
- 설정 화면의 "파일로 열기"는 `$CONFIG`의 실제 파일을 연다. "설정 폴더 열기" · "경로 복사" 액션 제공


| 범위 | 위치 (§5-0 XDG) | 형식 | 내용 |
|---|---|---|---|
| 전역 설정 | `$CONFIG/` | TOML | `settings.toml`(전역 설정 · 원격 호스트 · 외부 도구), `keymap.toml`, `themes/`, 개인 전용 프로젝트 설정 `projects/<slug>/` |
| 앱 데이터 | `$DATA/` | SQLite · 바이너리 | `state.db`(프로젝트 레지스트리 · 최근 목록), `projects/<id>/`(열린 탭 · 레이아웃 · 브레이크포인트 · 쿼리 히스토리), `extensions/`, `tools/`(LSP · DAP 설치본, §07 4.4), `relay/` |
| 상태 · 로그 | `$STATE/` | 텍스트 · SQLite | `logs/`, `backup/`(마이그레이션 원본), MCP 감사 로그 `mcp/audit.db`, 실행 중 앱의 MCP 소켓 `run/`([05-mcp.md](05-mcp.md) 「로컬 소켓과 헤드리스」) |
| 캐시 | `$CACHE/` | 다양 | 심볼 인덱스, registry 캐시, 원격 파일 캐시 — 지워도 재생성 |
| 프로젝트 공유 설정 | `<project>/.pitwall/project.toml`, `run/*.toml`, `db/*.toml` | TOML | 커밋 가능. 실행 구성, DB 연결(비밀 제외), 기본 외부 도구 고르기(명령은 정의하지 못한다 — 「`[open_with]`」) |
| 비밀 | OS 키체인 | | DB 비밀번호, SSH 패스프레이즈, 토큰. 설정 파일에는 `secret_ref`만 |

- **하이브리드 정책 (D9)**: `.pitwall/*.toml`은 커밋, 개인용 덮어쓰기는 `.pitwall/*.local.toml` — 프로젝트 생성 시 `.pitwall/.gitignore`에 `*.local.toml` 과 `.backup/` 자동 추가
- 병합 순서: 내장 기본값 → `.pitwall/*.toml` → `.pitwall/*.local.toml`
  - 단위는 파일 id(`.pitwall/` 기준 경로에서 `.toml`/`.local.toml` 을 뗀 것: `project`, `run/dev-server`). 읽는 범위는 `.pitwall/*.toml` 과 `run/` · `db/` 한 단계, 전역은 `$CONFIG/*.toml`(하위 폴더 제외)
  - 병합 규칙: 양쪽이 테이블이면 키별 재귀 병합, 그 밖(스칼라 · 배열 · 테이블의 배열 · 타입이 다른 값)은 위 레이어가 통째로 이긴다. 배열은 이어 붙이지 않는다. 위 레이어에서 키를 지우는 문법은 없다(`pw_core::config::merge`)
  - 파일 하나라도 읽기 · 파싱 · 마이그레이션에 실패하면 그 설정 전체가 실패다. 다시 읽기(파일 감시)에서 실패하면 **이전 설정을 유지**하고 `config.error` 이벤트를 낸다. 처음 읽기에서 실패하면 내장 기본값만으로 시작한다. 성공했고 값이 바뀐 파일 id 나 알림(마이그레이션 · 새 스키마)이 있으면 `config.changed`
  - 「전체 실패」 는 메모리의 설정에 대한 것이다. **디스크의 마이그레이션은 파일 단위로 일어난다** — 다른 파일이 깨져 있어도 앞선 파일의 마이그레이션은 쓰인다(원본은 백업)
- 워크트리 폴백: 프로젝트 루트가 연결 워크트리(`git worktree add`) 안이면 `.local.toml` 이 없을 때 메인 체크아웃의 같은 위치 `.pitwall/` 의 것을 쓴다. **파일 단위 폴백**이다 — 워크트리에 `<id>.local.toml` 이 있으면 그것만 쓰고 메인 쪽과 병합하지 않는다. 판정은 `.git` 파일 → gitdir 의 `commondir` 로 하고 git 을 실행하지 않는다(`pw_core::config::gitdir`). 워크트리로 보는 조건은 둘이다 — gitdir 의 `gitdir`(되돌림 포인터)이 그 `.git` 파일을 가리키고, gitdir 이 실제 경로로 **`<commondir>/worktrees/<이름>` 바로 아래**다(git 은 관리 폴더를 언제나 그 자리에 만든다). 둘째가 없으면 관리 폴더 · `commondir` · 되돌림 포인터를 모두 제 폴더 안에 둔 위조 `.git` 하나로 아무 폴더나 남의 저장소의 워크트리가 되어 그 프로젝트의 신뢰(§5-1 「신뢰」)와 파일 루트를 물려받는다
- 레포를 건드리고 싶지 않은 프로젝트는 "개인 전용" 모드로 전환 → 같은 파일 구조를 `$CONFIG/projects/<slug>/`에 저장 (사람이 편집하는 설정이므로 `$CONFIG`). 전환하면 저장소 `.pitwall/` 은 읽지 않는다
  - slug 는 프로젝트 레지스트리가 정하고 경로 한 칸(`[A-Za-z0-9._-]`, `.` 으로 시작하지 않음)만 받는다. 그 폴더의 마이그레이션 원본은 `$STATE/backup/projects/<slug>/`(`$CONFIG` 에 TOML 외 파일을 두지 않으므로)
  - 레지스트리가 만드는 slug 는 더 좁다: 이름에서 소문자로 만들고(대소문자를 무시하는 파일 시스템에서 `App` · `app` 이 같은 폴더다), 끝의 `.` 를 떼고, Windows 장치 이름(`con` 등)은 `_` 를 붙인다. 이름에서 아무것도 남지 않으면 `project`, 48바이트까지. 레지스트리에 있거나 **디스크에 그 폴더가 이미 있으면**(지운 프로젝트의 설정 등) `-2`, `-3` … 을 붙인다. 프로젝트 이름을 바꿔도 slug 는 그대로다. 구현: `pw_core::registry::slug`
- 프로젝트 레지스트리 `$DATA/state.db`(SQLite, `rusqlite`): 스키마 버전은 `PRAGMA user_version`, 단계별 마이그레이션은 단계마다 트랜잭션 하나. 처음 쓰일 때 연다(데스크톱은 시작 직후 백그라운드에서 열어 상태를 로그에 남긴다)
  - 여러 프로세스(워크트리마다 띄운 앱, 이후 CLI · relay)가 같은 파일을 쓴다. 쓰기와 마이그레이션 단계는 모두 `BEGIN IMMEDIATE` 로 쓰기 잠금을 먼저 잡고(잠겨 있으면 2초까지 기다린다), 마이그레이션은 그 잠금 안에서 버전을 다시 읽어 다른 프로세스가 이미 올린 단계를 건너뛴다. 단계의 쓰기 잠금을 2초 안에 못 잡았어도 그 사이 다른 프로세스가 단계를 올렸으면(버전을 다시 읽어 본다) 실패가 아니다
  - **state.db 는 WAL 저널로 연다**(쓰는 연결이 열 때 `journal_mode=wal`). 이유: 커밋의 디스크 동기화가 느린 디스크(Windows CI 실측 약 70ms/트랜잭션)에서 rollback 저널은 쉬지 않고 쓰는 프로세스가 잠금을 곧바로 다시 잡아, busy 폴링으로 기다리는 다른 프로세스가 2초 안에 끼어들지 못했다(동시에 처음 연 레지스트리 넷 중 셋이 메모리로 빠졌다). WAL 은 읽기가 쓰기를 기다리지 않고 커밋의 동기화도 한 번이다. **WAL 은 로컬 파일 시스템을 전제한다**(`$DATA` 는 로컬). 파일 옆에 `state.db-wal` · `state.db-shm` 이 생기고 마지막 연결이 닫히며 걷힌다. 비정상 종료로 둘이 남은 파일이 손상이면, 검사 연결이 닫히며 남은 WAL 프레임을 손상된 파일에 옮기고 둘을 지운 뒤 파일을 옮긴다(프레임은 `.corrupt` 안에 남는다). 다른 프로세스가 열고 있어 둘이 남아 있으면 파일과 함께 옮긴다 — Windows 에서는 다른 프로세스가 연 파일을 옮길 수 없어 복구가 실패하고 `memory` 로 뜬다
    - **`PRAGMA synchronous = NORMAL`**(WAL 로 바뀐 연결에만): WAL + 기본 `FULL` 로도 Windows 러너에서 커밋이 약 28ms(동시 쓰기 시험 한 라운드 400 커밋 11.25s)였고 쓰기끼리 같은 기아가 남았다. 기아는 커밋이 쓰기 잠금을 쥐는 시간에 달렸다 — 쓰는 쪽은 놓자마자 다시 잡고, 기다리는 쪽은 폴링 간격(1ms 로 줄여도)으로 그 틈을 못 잡는다. 트랜잭션 안에 지연을 넣어 잰 문턱은 약 5ms(2ms 통과 · 5ms 가끔 실패 · 10ms 이상 늘 실패)이고, 그 시간의 대부분이 커밋마다의 디스크 동기화다. `NORMAL` 은 체크포인트 때만 동기화한다. 파일은 깨지지 않고, 전원이 나가면 마지막 커밋 몇 개를 잃을 수 있다 — 레지스트리는 다시 등록하면 되는 데이터라 받아들인다. rollback 저널에서는 `NORMAL` 이 드물게 파일을 깨뜨릴 수 있어(SQLite 문서) 전환에 실패한 연결은 `FULL` 그대로다
    - 옛 빌드가 만든 rollback 파일은 처음 여는 쓰는 연결이 바꾼다(모드는 파일 머리에 남는다). 다른 연결이 읽기 트랜잭션 중이라 못 바꾸면 그때는 rollback 으로 쓰고 다음 열기에서 다시 해 본다. 옛 빌드의 연결은 다음 접근에서 WAL 로 따라온다
    - 쓸 수 없는 폴더의 WAL 파일은 `-shm` 을 만들 수 없어 보통 연결로 읽히지 않는다. 그때는 읽기 전용 연결로, 그것도 안 되면 바뀌지 않는 파일(`immutable`)로 연다 — 이 사용자는 그 폴더에 `-wal` 을 만들 수 없어 이 사용자의 프로세스는 연 동안 파일을 바꾸지 않는다. 폴더 소유자 같은 다른 사용자가 같은 파일에 쓰면 낡은 결과나 깨진 읽기를 받을 수 있다(같은 `$DATA` 를 다른 사용자와 나눠 쓸 때뿐이고, 손상 판정이 나도 쓰기 금지 폴더라 파일은 옮겨지지 않고 `memory` 로 떨어진다)
  - 열기 실패가 앱을 막지 않는다. 상태(`project.storage`)마다:
    - **손상**(SQLite 가 아님 · 무결성 검사 실패): 파일을 `state.db.corrupt`(겹치면 `.1` …)로 옮기고 새로 만든다(`recovered`, 쓸 수 있다)
    - **앱보다 새 스키마**(앱을 내린 경우) · **이 프로세스가 쓸 수 없는 파일 · 폴더**: 읽기 전용으로 연다(`readOnly`). 폴더는 권한 비트가 아니라 옆에 임시 파일을 실제로 만들어 보고 판정한다(다른 사용자 소유 폴더 · 읽기 전용 볼륨도 잡는다). 목록 · 최근 · 해석은 되고 쓰기는 거절한다. 이 앱의 질의로 읽히지 않으면 아래 메모리로
    - **잠김**(2초를 넘김) · 권한 · 그 밖: 파일을 건드리지 않고 빈 메모리 DB 로 뜬다(`memory`). 목록이 비어 보이고 쓰기는 거절한다. 잠김이면 **5초 간격으로** 파일을 다시 열어 보고(재시도 열기는 250ms 만 기다린다, 간격 안의 호출은 메모리 결과를 바로 돌려준다), 열리면 파일로 돌아간다
  - **`readOnly` · `memory` 에서 쓰기(생성 · 수정 · 삭제 · 「열었음」)는 성공처럼 돌려주지 않고 오류로 거절한다** — 쓴 것이 종료 때 사라지지 않게. 오류 코드는 전용 `STORAGE_UNAVAILABLE`(-32003, `data` 에 이유)이라 일반 내부 오류와 구분된다. **M0-12 는 이 코드를 받으면 `project.storage` 를 다시 불러 배너를 갱신한다**
  - 최신 버전 파일을 여는 것은 쓰기 잠금을 잡지 않는다(올릴 단계가 있을 때만 잡는다) — 다른 프로세스가 쓰는 중에도 기다리지 않고 연다
  - **M0-12 웰컴 화면은 `project.storage` 가 `ok` 가 아니면 그 이유(`reason`)를 배너로 보여 준다**(`memory` 의 빈 목록이 「프로젝트가 사라졌다」로 읽히지 않게). 지금은 데스크톱 시작 로그만 남긴다
  - 레지스트리에서 지워도 프로젝트 폴더 · `.pitwall/` · 개인 전용 설정 폴더는 남는다. 변경은 Host 범위 이벤트 `project.changed`(`kind`: created · updated · deleted · opened · trust)
- 기존 설정 임포트 (편의): `.idea/runConfigurations/*.xml`, `.idea/dataSources.xml`, `.vscode/launch.json`, `.vscode/tasks.json`

### 설정 파일 스키마 버전 (D23)
- 모든 설정 파일(`project.toml`, `run/*.toml`, `db/*.toml`, `settings.toml`, `keymap.toml`, `relay.toml`) 첫 줄에 `schema = <정수>`
- 앱이 구버전 파일을 읽으면 **자동 마이그레이션** → 원본은 `.pitwall/.backup/<파일>.v<N>`(프로젝트) / `$STATE/backup/`(전역)에 보관 후 새 형식으로 저장
- 커밋되는 `.pitwall/` 파일이 마이그레이션되면 알림 표시 (팀원과 앱 버전이 다를 수 있으므로 "변경 사항 커밋 필요" 안내)
- 앱보다 **새 버전** 스키마 파일은 읽기 전용으로 열고 업데이트 안내 (덮어쓰지 않음)
  - `schema` 가 없으면 v0. 보관 이름은 `<설정 폴더 기준 경로>.v<원래 버전>`, 같은 이름이 있으면 `.1`, `.2` … 를 붙여 덮지 않는다. 원본 보관이 실패하면 파일을 쓰지 않는다
  - 다시 쓸 때 주석 · 키 순서는 보존된다(`toml_edit`). 새로 넣는 `schema` 는 최상위 값의 맨 앞에 두므로 파일 머리 주석은 그 아래로 밀린다
  - 새 형식은 같은 폴더의 임시 파일에 쓰고 rename 으로 바꿔 끼운다. Windows 의 바꿔 끼우기(`MoveFileExW`)는 원자적이지 않아 그 순간 같은 파일을 여는 다른 로더가 「없음」 · 접근 거부 · 공유 위반을 잠깐 받는다 — Windows 에서만 설정 파일 읽기 · 쓰기가 이 오류를 2초까지(Go 와 같은 값) 다시 시도한다(Go cmd/go 의 robustio 와 같은 방법). 구현: `pw_core::config::robust`. 경로 해석(`canonicalize`)이 바꿔 끼워진 옛 파일을 집어 `$Extend\$Deleted` 아래 경로를 돌려주면 그 경로는 다시 생기지 않으므로, 해석과 권한 읽기를 한 번에 원래 경로부터 다시 한다. 설정 파일 하나의 io 오류는 그 설정 파일 경로로 보고되므로, 메시지에 실패한 연산과 실제 경로(대상 · 임시 파일 · 보관본)를 붙인다
- 마이그레이션은 버전별 순차 함수(`v1→v2→v3`)로 구현, 각 단계 테스트 픽스처 유지

### 로그 · 진단 (D22)
- 로그: `$STATE/logs/` 회전 로그(코어 · UI · LSP · DAP · Relay 별), 기본 7일 보관
- 사용 통계(텔레메트리) 수집 없음
- UI 언어: 전역 `settings.toml` 의 `[ui] language`(내장 기본값 `"system"` — OS 언어를 따른다). 고르기 · 쓰기 규칙은 [07-extensibility.md](07-extensibility.md) §4.1
- 업데이트 채널: 전역 `settings.toml` 의 `[update] channel`(`"stable"` 내장 기본값 | `"preview"`, 그 밖의 값은 경고를 남기고 stable). 매니페스트 위치 · 확인 시점은 [02-tech-stack.md](02-tech-stack.md) §3-1
- 크래시 리포트: 기본 꺼짐. 켜면 크래시 시 스택 · 버전 · OS만 전송(경로 · 코드 · 비밀 제외), 전송 전 내용 확인 가능. 켜는 값은 전역 `settings.toml` 의 `[diagnostics] crash_reports`(내장 기본값 false, 설정 파일이 깨졌으면 꺼짐으로 본다)
- 설정 읽기 실패(`config.error`)와 다시 읽기 알림(마이그레이션 · 새 스키마 · 끊긴 심링크)은 코어 로그에 경고로 남긴다. TOML 오류의 원문 발췌는 걷는다
- "진단 정보 복사": 버전 · OS · 활성 기능 · 최근 오류 로그를 클립보드로 (이슈 제보용)

### 프로젝트 설정 예시

```toml
# .pitwall/project.toml
schema = 1
name = "billing-api"
[languages]
enabled = ["kotlin", "sql"]

[runtime]            # 선택 — 생략 시 자동 감지
provider = "auto"    # auto | mise | shell | none (M1, 결정 M1-D15). direnv · asdf 는 그 제공자와 함께 더한다
[runtime.env]        # 수동 덮어쓰기 (제공자 결과 위에 적용)
JAVA_HOME = "/Library/Java/JavaVirtualMachines/temurin-21.jdk/Contents/Home"

[open_with]          # 이 프로젝트의 기본 외부 도구 — 등록된 도구 중에서 고르기만 한다(아래 「`[open_with]`」)
default = "zed"      # 외부 도구 id (04 F8)
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

### `[open_with]` — 외부 도구 설정 (04 F8)

도구 목록은 확장의 `[contributes.external_tools.<id>]`(기본 목록은 내장 확장 `extensions/external-tools`) 위에 **전역 설정**을 덮은 것이다. 구현: `crates/pw-open-with`

```toml
# $CONFIG/settings.toml (또는 settings.local.toml)
[open_with]
default = "zed"                                    # 기본 도구(⌘⌥E)

[open_with.tools.zed]                              # 등록된 도구 고치기 — 적은 키만 바꾼다
command = ["/opt/zed/bin/zed", "{project_root}"]

[open_with.tools.my-editor]                        # 새 도구 — label · command 가 있어야 한다
label = "My Editor"
command = ["my-editor", "{project_root}"]
file_command = ["my-editor", "--goto", "{file}:{line}"]
platforms = ["macos"]                              # (선택) 다른 플랫폼 도구를 이 OS 로 옮길 때도 쓴다
```

- 명령 형식은 매니페스트와 같다(argv 배열, 첫 요소에 변수 금지 — 04 F8 「기본 템플릿 예」, 07 「레지스트리」 표의 「외부 도구 명령」 행)
- 고칠 때 `command` 를 적으면 확장의 `file_command` 는 버린다(같이 적은 것만 남는다) — 실행 파일을 바꿨는데 파일을 열 때만 옛 실행 파일이 도는 일이 없게. `file_command` 만 적으면 그것만 바꾼다
- 형식이 틀린 도구는 목록에 「쓸 수 없음」 으로 남고 실행하면 그 이유를 알린다. 확장의 명령으로 조용히 물러나지 않는다 — 고치려던 명령 대신 옛 명령이 돌면 안 된다
- **프로젝트 설정**(`.pitwall/project.toml` · `.local.toml` · 개인 전용 `project.toml`)은 `[open_with] default` 로 **등록된 도구 중에서 고르기만** 한다. 명령을 정의하거나 바꿀 수 없다 — 저장소를 클론해 열기만 했는데 Pitwall 이 그 저장소가 정한 명령을 실행하면 안 된다(MCP 노출의 「공유 설정은 좁히기만」 과 같은 원리, 05 「툴 노출 설정」). 프로젝트 설정의 `[open_with.tools]` 는 경고하고 버린다
  - 프로젝트 설정은 **디스크에 쓰지 않고** 읽는다 — 외부 도구로 열었다고 커밋된 파일이 마이그레이션되거나 `.backup/` 이 생기지 않는다
- 기본 도구: 프로젝트 설정 → 전역 설정 순. 고른 도구가 이 OS 의 목록에 없으면 다음 것으로 넘어간다. 아무것도 없으면 **`⌘⌥E` 가 「다음에서 열기」 팝업을 열고** 기본 도구가 없다(또는 고른 도구를 쓸 수 없다)고 알린다 — 설치됐는지 모르는 도구(예: Orca)를 내장 기본값으로 두지 않는다

## 5-1. 런타임 환경 (D26)

GUI 앱은 터미널과 환경이 달라 프로젝트가 고정한 런타임 버전(Node/Java/Python 등)을 모른다. Pitwall은 **런타임 버전을 직접 설치 · 관리하지 않고**, 외부 도구가 계산한 환경을 받아 프로세스를 띄울 때 적용한다.

### 런타임 제공자 (Runtime Provider)

| 순서 | 제공자 | 감지 조건 | 환경 얻는 방법 |
|---|---|---|---|
| 1 | **mise** (우선 지원) | `mise` 실행 파일 존재 + 프로젝트에 `mise.toml` / `.mise.toml` / `.tool-versions` 등 | `mise env --json` (cwd = 대상 디렉터리) |
| 2 | direnv | `.envrc` 존재 + `direnv` 설치 | `direnv export json` |
| 3 | asdf | `.tool-versions` + `asdf` 설치 (mise 없을 때) | `asdf` shim 경로를 PATH에 추가 |
| 4 | **로그인 셸** (기본 대체) | 항상 | `$SHELL -lic env` 결과 (Orca의 PATH 가져오기와 같은 방식). 앱 시작을 막지 않게 처음 쓸 때 한 번 가져온다 |
| 5 | 수동 지정 | `.pitwall/project.toml`의 `[runtime]` | 설정값을 위 결과에 덮어씀 |

- 자동 선택이 기본, 프로젝트 설정에서 제공자 고정 가능 (`provider = "auto" | "mise" | "shell" | "none"` — M1 이 읽는 값, 결정 M1-D15(12-m1-plan.md §4). `"direnv"` · `"asdf"` 는 그 제공자가 들어올 때 더한다)
- mise는 **필수 의존이 아님** — 없으면 로그인 셸 환경으로 동작
- 결과는 `RuntimeEnv { path, vars, tools: [{name, version, source_file}] }`로 통일
- **지금(`crates/pw-runtime`, M1-03)은 4번 로그인 셸만** 있다 — 외부 도구 실행(04 F8)이 쓰고, 터미널(M1-08)이 이어 쓴다. mise 는 M1-16 이 같은 trait(`RuntimeProvider`)로 더한다. Finder · Dock · `open` 으로 띄운 GUI 앱의 환경은 launchd 의 것(PATH `/usr/bin:/bin:/usr/sbin:/sbin`)이라 `zed` · `code` · `idea` 를 못 찾고 셸에서 정한 `JAVA_HOME` 도 없다(터미널에서 띄운 앱은 터미널 환경을 물려받아 가려진다)
  - 환경은 층으로 쌓인다: 앱 프로세스의 환경 → 로그인 셸(언제나) → 디렉터리에 맞는 제공자 하나(mise 등). 프로젝트 코드를 돌리는 제공자(`runs_project_code`)는 신뢰한 프로젝트에서만 고른다 — 신뢰 여부는 `Runtime` 의 입력이고 지금은 모두 미신뢰다(M1-05 가 채운다). 아래 층이 실패하면 그 아래 층으로 물러난다
  - `<셸> -l -i -c "printf 표식; /usr/bin/env -0; printf 표식"` 의 두 표식 사이의 **환경 전체**를 읽는다 — 시작 파일이 stdout 에 찍는 잡음은 표식 바깥이라 버려진다. 셸 고유 변수 `PWD` · `SHLVL` · `_` 와 표식 `PITWALL_RESOLVING_ENVIRONMENT` 는 버린다. 셸은 `$SHELL`, 없으면 계정의 셸(`getpwuid_r`). 셸은 디렉터리와 무관하게 한 번 돈다(시작 파일은 사용자 단위다)
  - 셸은 `setsid` 로 제어 터미널 없이 돈다(대화형 셸이 터미널을 잡으려다 멈추지 않게). 시작 파일이 무거운 일을 건너뛸 수 있게 `PITWALL_RESOLVING_ENVIRONMENT=1` 을 준다
  - 5초 안에 환경을 찍지 않으면 셸의 세션을 통째로 죽이고, 실패 · 시간 초과 · PATH 없음 · 출력 64 MiB 초과면 앱 프로세스의 환경으로 물러난다(`runtime.get` 의 `status` 가 `fallback`). **물러난 결과는 캐시하지 않아 다음 요청이 셸을 다시 돌린다** — 셸 설정을 고치면 앱을 다시 띄우지 않아도 된다. 무엇을 썼는지는 로그(`environment from the login shell` · `using the process environment`)로 남는다
  - **단, 시간 초과만 30초 냉각한다**: 그동안은 셸을 다시 돌리지 않고 물러난 결과를 곧바로 주며, 냉각이 끝난 뒤 첫 요청에서 다시 시도한다. 「환경 다시 읽기」 는 냉각을 푼다. 왜: 시작 파일이 대화형 셸에서 `exec fish` · `exec tmux` 처럼 멈추면 매번 시간 초과라, 냉각이 없으면 요청마다(터미널이면 새 탭마다) 5초를 기다린다. 셸을 못 띄우는 것 · PATH 없음처럼 곧바로 끝나는 실패는 냉각하지 않는다
  - 셸 출력은 부른 스레드가 `poll(2)` 로 시간 제한까지만 읽고, 둘째 표식이 보이면 곧바로 파이프를 닫는다 — 시작 파일이 띄운 다른 세션의 프로세스가 stdout 을 쥐고 있어도 기다리거나 읽기 스레드를 남기지 않는다. 표식은 새로 읽은 구간만 찾는다(잡음이 길어도 출력 길이에 선형)
  - 앱 시작이 아니라 **처음 쓸 때** 가져오고(「다음에서 열기」 팝업을 열면 미리 시작한다), 성공한 결과는 (프로젝트, 디렉터리)마다 「환경 다시 읽기」 전까지 캐시한다. 동시에 여러 곳이 물어도 셸은 한 번 돈다
  - 자식은 앱 프로세스의 환경을 물려받지 않고 이 환경 전체만 받는다(PATH 는 절대 경로 항목만, `crates/pw-open-with`)
  - RPC: `runtime.get { projectId, dir? }` → `{ provider, source, status, tools }`(`dir` 이 없으면 주 루트). `runtime.reload { projectId }`(액션 「런타임 환경 다시 읽기」) — 로그인 셸 환경과 모든 디렉터리의 캐시를 잊고, 캐시가 있던 프로젝트와 부른 프로젝트에 `runtime.changed { reason: "reload" }` 를 한 번씩 낸다(프로젝트 범위). 셸은 백그라운드로 곧 다시 돌고, 다음 요청은 그 결과를 기다린다. `runtime.get` 의 `dir` 은 레지스트리와 같은 규칙으로 정규화하고, 프로젝트 루트나 그 루트를 담은 연결 워크트리 안만 받는다(밖이면 invalid params) — 제공자가 호출자가 고른 아무 폴더에서 돌지 않게

### 적용 대상
모든 프로세스 실행은 `runtime` 모듈을 거친다: **내장 터미널, LSP 서버, DAP 어댑터, 실행 구성, 태스크, MCP를 통한 실행, git 훅 실행 환경**

### 캐시 · 무효화
- 키: (프로젝트, 디렉터리). 모노레포 하위 폴더별로 다른 버전 가능
- 무효화: `mise.toml` · `.tool-versions` · `.envrc` · `.nvmrc` 등 버전 파일 변경(파일 감시), 사용자 "환경 다시 읽기" 액션
- 런타임 변경 시 `runtime.changed` 이벤트 → 영향받는 LSP 재시작 제안, 새 터미널부터 적용

### 신뢰
- mise 설정은 hook · env 스크립트를 실행할 수 있음 → **신뢰된 프로젝트(F1-7)에서만** 제공자 실행
- 신뢰 전: 로그인 셸 환경만 사용, 상태바에 "런타임 환경 미적용(신뢰 필요)" 표시
- 저장(M1-05, 구현 `pw_core::registry::trust`): `state.db` v2 의 `project_trust`(프로젝트마다 한 행, 프로젝트를 지우면 함께 지워진다). 행이 없으면 **아직 묻지 않음**이고 미신뢰로 다룬다 — v1 의 기존 프로젝트는 행 없이 올라와 다음에 열 때 한 번 묻는다. RPC 는 `project.trust`(read, 모든 role) · `project.setTrust`(write, owner) · `project.claimTrustPrompt`(write, owner). **MCP 툴로 내놓지 않는다**(신뢰는 사람만 바꾼다). `readOnly` · `memory` 에서 쓰기 둘은 `STORAGE_UNAVAILABLE`
- **워크트리는 메인 저장소 프로젝트의 신뢰를 따른다**: 등록되지 않은 워크트리는 해석이 이미 메인 저장소 프로젝트로 연다. 워크트리를 따로 등록한 프로젝트는 그 루트를 메인 체크아웃 쪽으로 옮겨 그곳을 포함하는 등록 루트(해석 2 단계와 같은 규칙)의 프로젝트가 신뢰를 가진다(`ownerId`). 읽기 · 바꾸기 · 묻기 모두 그 프로젝트의 행이다. 메인 저장소가 등록되지 않았으면 자기 신뢰다
- 확인 다이얼로그는 **한 번만**: 창은 띄우기 전에 `project.claimTrustPrompt` 를 부르고, 쓰기 트랜잭션 안에서 「묻지 않음」을 「미신뢰」로 바꾼 호출만 `true` 를 받는다 — 같은 프로젝트의 창 여럿(다른 프로세스 포함)이 동시에 열려도 하나만 묻는다. 답하지 않고 닫으면(Esc · 바깥 누르기) 미신뢰로 남고 상태바 항목 · 「이 프로젝트 신뢰…」 액션에서 다시 연다. **확인 창이 뜨기 전에 창이 닫히거나 웹뷰가 다시 로드돼도 같다** — 차례를 잡을 때 이미 미신뢰로 기록됐으므로 다시 묻지 않고 상태바에서 연다. 같은 값으로 바꾸기(이미 미신뢰인데 「신뢰하지 않음」)는 쓰지도 이벤트를 내지도 않는다
- 저장할 수 없는 레지스트리(`project.storage` 가 readOnly · memory)에서는 묻지 않고, 상태바 항목은 누를 수 없는 「런타임 환경 미적용(신뢰를 저장할 수 없음)」 이 되며 신뢰 · 해제 액션은 꺼진다. 저장이 `STORAGE_UNAVAILABLE` 로 거절되면 창이 `project.storage` 를 다시 읽는다
- 신뢰를 바꾸면(`project.changed` 의 `trust`) 앱이 그 신뢰를 쓰는 프로젝트들(신뢰를 가진 프로젝트와 그 신뢰를 따르는 워크트리 프로젝트, `Registry::trust_followers`)의 디렉터리 캐시만 버리고 각각 `runtime.changed`(`reason: trust`)를 낸다(`Runtime::trust_changed`) — 새 터미널부터 제공자 환경이 붙거나 빠진다. 로그인 셸 환경과 다른 프로젝트의 캐시는 그대로다. 수동 「환경 다시 읽기」(`reason: reload`)와 이유로 구분된다. 연결은 `apps/desktop/src-tauri/src/trust.rs`
- 런타임 캐시 키는 (프로젝트, 디렉터리, **계산할 때 판정한 신뢰**)다. 꺼낼 때마다 지금 신뢰로 키를 만들므로, 신뢰를 가진 프로젝트가 바뀌어(메인 저장소 프로젝트의 등록 · 삭제 · 루트 변경 — 그때는 `trust` 이벤트가 없다) 신뢰가 달라져도 옛 신뢰로 만든 환경을 주지 않는다. 레지스트리 이벤트를 놓치면(`Lagged`) 무엇이 바뀌었는지 모르므로 전역 다시 읽기(`Runtime::reload_all`)를 한다
- mise 자체 신뢰(`mise trust`)가 안 된 경우 안내 + 버튼으로 실행 (사용자 확인 후)

### 미설치 버전
- 제공자가 "설치되지 않은 버전"을 보고하면 알림: [`mise install` 실행] (터미널 탭에서 실행해 출력 표시)

### 원격
- Relay도 같은 `runtime` 모듈 사용 → 원격 머신의 mise/셸 환경 기준

## 6. 원격 연결 구조

상세는 [08-remote-mobile.md](08-remote-mobile.md).
- 원격 머신에 `pitwall-relay`를 설치(서비스 상주)하고 QR/코드로 페어링
- 연결: 직접(QUIC, LAN/Tailscale) → SSH stdio 순으로 시도, 모두 Noise로 인증 · 암호화. 중계 서버(Hub)는 두지 않음
- Relay는 상주 데몬이므로 연결이 끊겨도 실행 중 프로세스 · 로그 · 디버그 세션 유지
- 포트 포워딩: 원격 웹 서버 · 디버그 포트를 로컬로 포워딩 (DAP attach용)
- DB는 원격 프로젝트면 Relay에서 직접 연결, 로컬 프로젝트는 SSH 터널 옵션 제공

## 7. 보안


- MCP HTTP 서버는 `127.0.0.1`에만 바인드 + 기동 시 생성되는 Bearer 토큰 필수
- 위험 작업(파괴적 git, DB 쓰기, 실행 구성의 임의 커맨드 실행)은 MCP 경유 시 UI 승인 필요 (→ 05-mcp.md)
- Relay ↔ 기기 간 통신은 Noise 인증 · 암호화, Relay는 Tailscale/LAN 인터페이스에만 바인드(기본)
- RPC 메서드마다 필요 권한(role) 메타데이터 → Relay가 페어링 기기의 role로 검사 (readonly 등 제한 role)
- 파일 RPC(`fs.*`, `crates/pw-fs`)는 프로젝트 루트(활성 워크트리 안의 루트 포함) 안만 다룬다(12-m1-plan.md 결정 M1-D8). 경로는 절대 경로만 받아 레지스트리와 같은 정규화(`pw_core::registry::path::normalize`)를 지난다. **읽기**(읽기 · 목록 · 상태)는 심링크를 끝까지 푼 실제 경로가 루트 안일 때만이다 — 루트 안 심링크는 실제 대상이 루트 안일 때만 따르고, 밖을 가리키는 링크는 목록에 `outside` 로 표시만 하고 따르지 않는다(클론한 저장소의 링크 하나로 디스크를 열지 않게). **쓰기**는 실제 경로가 루트 안일 때만이고, 심링크는 대상 파일을 바꾼다(대상이 없거나 고리인 링크에는 쓰지 않는다). 같은 파일의 「version 확인 → rename」 은 Host 안에서 파일별로 묶여 같은 기대 `version` 의 두 쓰기 중 하나만 성공한다. 판정과 open 사이에 경로가 루트 밖 링크로 바뀌는 읽기 경합은 연 핸들의 실제 경로(macOS `F_GETPATH` · Linux `/proc/self/fd` · Windows `GetFinalPathNameByHandleW`)를 다시 판정해 버린다. 한계: 다른 프로세스의 쓰기와는 묶이지 않고, 판정과 rename 사이에 로컬 프로세스가 경로의 폴더를 루트 밖 링크로 바꾸는 쓰기 경합과 폴더 목록(`read_dir`)의 같은 경합은 막지 않으며(폴더 핸들 기준 `openat` · `renameat` 가 필요하다), 핸들 경로를 얻지 못하는 플랫폼에서는 읽기도 판정만 한다
- 확장은 WASM 샌드박스 + 설치 시 승인한 capability만 사용
- 프로젝트 설정의 실행 커맨드는 "신뢰된 프로젝트"에서만 실행 (VS Code Workspace Trust와 유사)
