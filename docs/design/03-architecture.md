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
- 장기 작업은 `Task` 추상(진행률, 취소)으로 통일 → UI 상태바 / MCP 둘 다 표시 가능

## 3. 프로세스 · 창 모델

- **한 프로세스, 프로젝트당 한 창** (IntelliJ와 동일한 감각)
- 프로젝트 창을 닫으면 해당 프로젝트의 LSP/DAP/실행 프로세스 정리 (실행 중 프로세스는 확인 후)
- 단일 인스턴스: 두 번째 실행(`pitwall path`)은 기존 인스턴스에 인자와 cwd 를 전달하고 종료. 첫 실행도 같은 열기 경로를 지난다(04 F2 「명령줄 규칙」). 키는 앱 identifier 하나다 — release 는 그대로 하나, **debug 빌드는 identifier 에 「빌드한 크레이트 폴더 + `$DATA`」의 해시를 붙여** 워크트리마다 · 데이터 폴더마다 갈린다(여러 워크트리의 dev 앱이 서로에게 전달되지 않게). 그래서 Windows · Linux 의 dev 빌드는 웹뷰 데이터 폴더(localStorage)도 키마다 따로다
- 웰컴 창: 최근 프로젝트, 원격 호스트, 새 프로젝트

## 4. RPC 설계

- JSON-RPC 2.0, 메서드명 `<module>.<action>` (예: `git.log`, `db.query.execute`)
- Host Router 는 연결 단위로 라우팅한다. 클라이언트는 `projectId`(또는 프로젝트가 없는 창은 `hostId`)로 Host 연결을 열고, 그 연결의 요청 · 스트림 · 구독 id 는 그 Host 세션 안에서만 유효하다. 기능 메서드의 params 에는 여전히 `projectId` 를 넣어 Host 안에서 `ProjectContext` 를 고른다. 여러 Host 의 프로젝트를 한 세션에서 다루는 클라이언트(MCP 서버)는 Host 별 연결을 하나씩 두고 요청마다 `projectId` 로 연결을 고른다. 프로젝트를 다른 Host 로 다시 묶으면 그 프로젝트의 기존 연결은 닫고 새로 연다.
- 대용량 응답(로그, 쿼리 결과, git log)은 **스트림**: `streamId`를 반환하고 `stream.chunk` 알림으로 전달, 백프레셔는 credit 방식
- 이벤트 구독: `events.subscribe({ projectId, topics })`. `projectId` 를 빼면 Host 범위(프로젝트에 속하지 않는 이벤트 — 전역 설정 변경 등)를 받는다. 프로젝트 없는 창은 이것으로 구독한다
- 타입 정의 단일 소스: Rust 구조체 → specta(TS) / schemars(JSON Schema, MCP)

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


| 범위 | 위치 | 형식 | 내용 |
|---|---|---|---|
| 범위 | 위치 (§5-0 XDG) | 형식 | 내용 |
|---|---|---|---|
| 전역 설정 | `$CONFIG/` | TOML | `settings.toml`(전역 설정 · 원격 호스트 · 외부 도구), `keymap.toml`, `themes/`, 개인 전용 프로젝트 설정 `projects/<slug>/` |
| 앱 데이터 | `$DATA/` | SQLite · 바이너리 | `state.db`(프로젝트 레지스트리 · 최근 목록), `projects/<id>/`(열린 탭 · 레이아웃 · 브레이크포인트 · 쿼리 히스토리), `extensions/`, `tools/`(LSP · DAP 설치본, §07 4.4), `relay/` |
| 상태 · 로그 | `$STATE/` | 텍스트 | `logs/`, `backup/`(마이그레이션 원본), 감사 로그 |
| 캐시 | `$CACHE/` | 다양 | 심볼 인덱스, registry 캐시, 원격 파일 캐시 — 지워도 재생성 |
| 프로젝트 공유 설정 | `<project>/.pitwall/project.toml`, `run/*.toml`, `db/*.toml` | TOML | 커밋 가능. 실행 구성, DB 연결(비밀 제외), 외부 도구 오버라이드 |
| 비밀 | OS 키체인 | | DB 비밀번호, SSH 패스프레이즈, 토큰. 설정 파일에는 `secret_ref`만 |

- **하이브리드 정책 (D9)**: `.pitwall/*.toml`은 커밋, 개인용 덮어쓰기는 `.pitwall/*.local.toml` — 프로젝트 생성 시 `.pitwall/.gitignore`에 `*.local.toml` 과 `.backup/` 자동 추가
- 병합 순서: 내장 기본값 → `.pitwall/*.toml` → `.pitwall/*.local.toml`
  - 단위는 파일 id(`.pitwall/` 기준 경로에서 `.toml`/`.local.toml` 을 뗀 것: `project`, `run/dev-server`). 읽는 범위는 `.pitwall/*.toml` 과 `run/` · `db/` 한 단계, 전역은 `$CONFIG/*.toml`(하위 폴더 제외)
  - 병합 규칙: 양쪽이 테이블이면 키별 재귀 병합, 그 밖(스칼라 · 배열 · 테이블의 배열 · 타입이 다른 값)은 위 레이어가 통째로 이긴다. 배열은 이어 붙이지 않는다. 위 레이어에서 키를 지우는 문법은 없다(`pw_core::config::merge`)
  - 파일 하나라도 읽기 · 파싱 · 마이그레이션에 실패하면 그 설정 전체가 실패다. 다시 읽기(파일 감시)에서 실패하면 **이전 설정을 유지**하고 `config.error` 이벤트를 낸다. 처음 읽기에서 실패하면 내장 기본값만으로 시작한다. 성공했고 값이 바뀐 파일 id 나 알림(마이그레이션 · 새 스키마)이 있으면 `config.changed`
  - 「전체 실패」 는 메모리의 설정에 대한 것이다. **디스크의 마이그레이션은 파일 단위로 일어난다** — 다른 파일이 깨져 있어도 앞선 파일의 마이그레이션은 쓰인다(원본은 백업)
- 워크트리 폴백: 프로젝트 루트가 연결 워크트리(`git worktree add`) 안이면 `.local.toml` 이 없을 때 메인 체크아웃의 같은 위치 `.pitwall/` 의 것을 쓴다. **파일 단위 폴백**이다 — 워크트리에 `<id>.local.toml` 이 있으면 그것만 쓰고 메인 쪽과 병합하지 않는다. 판정은 `.git` 파일 → gitdir 의 `commondir` 로 하고 git 을 실행하지 않는다
- 레포를 건드리고 싶지 않은 프로젝트는 "개인 전용" 모드로 전환 → 같은 파일 구조를 `$CONFIG/projects/<slug>/`에 저장 (사람이 편집하는 설정이므로 `$CONFIG`). 전환하면 저장소 `.pitwall/` 은 읽지 않는다
  - slug 는 프로젝트 레지스트리가 정하고 경로 한 칸(`[A-Za-z0-9._-]`, `.` 으로 시작하지 않음)만 받는다. 그 폴더의 마이그레이션 원본은 `$STATE/backup/projects/<slug>/`(`$CONFIG` 에 TOML 외 파일을 두지 않으므로)
  - 레지스트리가 만드는 slug 는 더 좁다: 이름에서 소문자로 만들고(대소문자를 무시하는 파일 시스템에서 `App` · `app` 이 같은 폴더다), 끝의 `.` 를 떼고, Windows 장치 이름(`con` 등)은 `_` 를 붙인다. 이름에서 아무것도 남지 않으면 `project`, 48바이트까지. 레지스트리에 있거나 **디스크에 그 폴더가 이미 있으면**(지운 프로젝트의 설정 등) `-2`, `-3` … 을 붙인다. 프로젝트 이름을 바꿔도 slug 는 그대로다. 구현: `pw_core::registry::slug`
- 프로젝트 레지스트리 `$DATA/state.db`(SQLite, `rusqlite`): 스키마 버전은 `PRAGMA user_version`, 단계별 마이그레이션은 단계마다 트랜잭션 하나. 처음 쓰일 때 연다(데스크톱은 시작 직후 백그라운드에서 열어 상태를 로그에 남긴다)
  - 여러 프로세스(워크트리마다 띄운 앱, 이후 CLI · relay)가 같은 파일을 쓴다. 쓰기와 마이그레이션 단계는 모두 `BEGIN IMMEDIATE` 로 쓰기 잠금을 먼저 잡고(잠겨 있으면 2초까지 기다린다), 마이그레이션은 그 잠금 안에서 버전을 다시 읽어 다른 프로세스가 이미 올린 단계를 건너뛴다
  - 열기 실패가 앱을 막지 않는다. 상태(`project.storage`)마다:
    - **손상**(SQLite 가 아님 · 무결성 검사 실패): 파일을 `state.db.corrupt`(겹치면 `.1` …)로 옮기고 새로 만든다(`recovered`, 쓸 수 있다)
    - **앱보다 새 스키마**(앱을 내린 경우) · **이 프로세스가 쓸 수 없는 파일 · 폴더**: 읽기 전용으로 연다(`readOnly`). 폴더는 권한 비트가 아니라 옆에 임시 파일을 실제로 만들어 보고 판정한다(다른 사용자 소유 폴더 · 읽기 전용 볼륨도 잡는다). 목록 · 최근 · 해석은 되고 쓰기는 거절한다. 이 앱의 질의로 읽히지 않으면 아래 메모리로
    - **잠김**(2초를 넘김) · 권한 · 그 밖: 파일을 건드리지 않고 빈 메모리 DB 로 뜬다(`memory`). 목록이 비어 보이고 쓰기는 거절한다. 잠김이면 **5초 간격으로** 파일을 다시 열어 보고(재시도 열기는 250ms 만 기다린다, 간격 안의 호출은 메모리 결과를 바로 돌려준다), 열리면 파일로 돌아간다
  - **`readOnly` · `memory` 에서 쓰기(생성 · 수정 · 삭제 · 「열었음」)는 성공처럼 돌려주지 않고 오류로 거절한다** — 쓴 것이 종료 때 사라지지 않게. 오류 코드는 전용 `STORAGE_UNAVAILABLE`(-32003, `data` 에 이유)이라 일반 내부 오류와 구분된다. **M0-12 는 이 코드를 받으면 `project.storage` 를 다시 불러 배너를 갱신한다**
  - 최신 버전 파일을 여는 것은 쓰기 잠금을 잡지 않는다(올릴 단계가 있을 때만 잡는다) — 다른 프로세스가 쓰는 중에도 기다리지 않고 연다
  - **M0-12 웰컴 화면은 `project.storage` 가 `ok` 가 아니면 그 이유(`reason`)를 배너로 보여 준다**(`memory` 의 빈 목록이 「프로젝트가 사라졌다」로 읽히지 않게). 지금은 데스크톱 시작 로그만 남긴다
  - 레지스트리에서 지워도 프로젝트 폴더 · `.pitwall/` · 개인 전용 설정 폴더는 남는다. 변경은 Host 범위 이벤트 `project.changed`(`kind`: created · updated · deleted · opened)
- 기존 설정 임포트 (편의): `.idea/runConfigurations/*.xml`, `.idea/dataSources.xml`, `.vscode/launch.json`, `.vscode/tasks.json`

### 설정 파일 스키마 버전 (D23)
- 모든 설정 파일(`project.toml`, `run/*.toml`, `db/*.toml`, `settings.toml`, `keymap.toml`, `relay.toml`) 첫 줄에 `schema = <정수>`
- 앱이 구버전 파일을 읽으면 **자동 마이그레이션** → 원본은 `.pitwall/.backup/<파일>.v<N>`(프로젝트) / `$STATE/backup/`(전역)에 보관 후 새 형식으로 저장
- 커밋되는 `.pitwall/` 파일이 마이그레이션되면 알림 표시 (팀원과 앱 버전이 다를 수 있으므로 "변경 사항 커밋 필요" 안내)
- 앱보다 **새 버전** 스키마 파일은 읽기 전용으로 열고 업데이트 안내 (덮어쓰지 않음)
  - `schema` 가 없으면 v0. 보관 이름은 `<설정 폴더 기준 경로>.v<원래 버전>`, 같은 이름이 있으면 `.1`, `.2` … 를 붙여 덮지 않는다. 원본 보관이 실패하면 파일을 쓰지 않는다
  - 다시 쓸 때 주석 · 키 순서는 보존된다(`toml_edit`). 새로 넣는 `schema` 는 최상위 값의 맨 앞에 두므로 파일 머리 주석은 그 아래로 밀린다
- 마이그레이션은 버전별 순차 함수(`v1→v2→v3`)로 구현, 각 단계 테스트 픽스처 유지

### 로그 · 진단 (D22)
- 로그: `$STATE/logs/` 회전 로그(코어 · UI · LSP · DAP · Relay 별), 기본 7일 보관
- 사용 통계(텔레메트리) 수집 없음
- UI 언어: 전역 `settings.toml` 의 `[ui] language`(내장 기본값 `"system"` — OS 언어를 따른다). 고르기 · 쓰기 규칙은 [07-extensibility.md](07-extensibility.md) §4.1
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
provider = "auto"    # auto | mise | direnv | asdf | shell | none
[runtime.env]        # 수동 덮어쓰기 (제공자 결과 위에 적용)
JAVA_HOME = "/Library/Java/JavaVirtualMachines/temurin-21.jdk/Contents/Home"

[open_with]          # 프로젝트별 외부 도구 오버라이드
default = "orca"     # 외부 도구 id (04 F8)
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

## 5-1. 런타임 환경 (D26)

GUI 앱은 터미널과 환경이 달라 프로젝트가 고정한 런타임 버전(Node/Java/Python 등)을 모른다. Pitwall은 **런타임 버전을 직접 설치 · 관리하지 않고**, 외부 도구가 계산한 환경을 받아 프로세스를 띄울 때 적용한다.

### 런타임 제공자 (Runtime Provider)

| 순서 | 제공자 | 감지 조건 | 환경 얻는 방법 |
|---|---|---|---|
| 1 | **mise** (우선 지원) | `mise` 실행 파일 존재 + 프로젝트에 `mise.toml` / `.mise.toml` / `.tool-versions` 등 | `mise env --json` (cwd = 대상 디렉터리) |
| 2 | direnv | `.envrc` 존재 + `direnv` 설치 | `direnv export json` |
| 3 | asdf | `.tool-versions` + `asdf` 설치 (mise 없을 때) | `asdf` shim 경로를 PATH에 추가 |
| 4 | **로그인 셸** (기본 대체) | 항상 | 앱 시작 시 `$SHELL -lic env` 결과 (Orca의 PATH 가져오기와 같은 방식) |
| 5 | 수동 지정 | `.pitwall/project.toml`의 `[runtime]` | 설정값을 위 결과에 덮어씀 |

- 자동 선택이 기본, 프로젝트 설정에서 제공자 고정 가능 (`provider = "mise" | "direnv" | "asdf" | "shell" | "none"`)
- mise는 **필수 의존이 아님** — 없으면 로그인 셸 환경으로 동작
- 결과는 `RuntimeEnv { path, vars, tools: [{name, version, source_file}] }`로 통일

### 적용 대상
모든 프로세스 실행은 `runtime` 모듈을 거친다: **내장 터미널, LSP 서버, DAP 어댑터, 실행 구성, 태스크, MCP를 통한 실행, git 훅 실행 환경**

### 캐시 · 무효화
- 키: (프로젝트, 디렉터리). 모노레포 하위 폴더별로 다른 버전 가능
- 무효화: `mise.toml` · `.tool-versions` · `.envrc` · `.nvmrc` 등 버전 파일 변경(파일 감시), 사용자 "환경 다시 읽기" 액션
- 런타임 변경 시 `runtime.changed` 이벤트 → 영향받는 LSP 재시작 제안, 새 터미널부터 적용

### 신뢰
- mise 설정은 hook · env 스크립트를 실행할 수 있음 → **신뢰된 프로젝트(F1-7)에서만** 제공자 실행
- 신뢰 전: 로그인 셸 환경만 사용, 상태바에 "런타임 환경 미적용(신뢰 필요)" 표시
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
- 확장은 WASM 샌드박스 + 설치 시 승인한 capability만 사용
- 프로젝트 설정의 실행 커맨드는 "신뢰된 프로젝트"에서만 실행 (VS Code Workspace Trust와 유사)
