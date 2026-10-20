# 02. 기술 스택

## 1. 결론

| 영역 | 선택 | 비고 |
|---|---|---|
| 앱 셸 | **Tauri 2** | 네이티브 WebView, 작은 번들 · 낮은 메모리 |
| 코어 언어 | **Rust** (tokio) | git/DB/LSP/DAP/PTY/SSH 생태계가 모두 Rust에 존재 |
| UI | **React 19 + TypeScript + Vite** | 가상화 리스트 · 그리드 · 에디터 생태계 |
| UI 상태 | Zustand + TanStack Query | 코어 이벤트 스트림 → 쿼리 캐시 무효화 |
| UI 컴포넌트 | Radix UI primitives + Tailwind v4 (shadcn/ui 방식) | 키보드 접근성, IDE형 밀도 높은 레이아웃 — 전체 UI 스택은 [10-ui.md](10-ui.md) §3 |
| 레이아웃(도킹) | dockview | 탭/스플릿/도킹 패널 |
| 에디터 | **CodeMirror 6** | 경량, 뷰어 우선. Monaco 대비 번들 · 메모리 작음 |
| 구문 강조 | 코어: **tree-sitter** + WASM 문법(wasmtime) / UI: 코어가 준 화면 범위의 캡처 구간을 CodeMirror decoration 으로 그린다. Lezer 문법은 쓰지 않는다 | 언어 범위는 tree-sitter 기준. 되돌림 조건 등 상세는 [12-m1-plan.md](12-m1-plan.md) M1-D2 |
| Diff 뷰 | @codemirror/merge | side-by-side, inline |
| 터미널 | xterm.js + `portable-pty` | 실행 콘솔/터미널 공용 |
| Git 읽기 | **gitoxide (`gix`)** | log/graph/status/blame/diff를 빠르게 |
| Git 쓰기 | **git CLI** | commit/merge/rebase/checkout/push — 훅, 서명, credential helper 존중 |
| DB | **sqlx** (Postgres/MySQL/SQLite) + **`redis-rs`** (Redis) | 드라이버 추상화 계층 위에 얹음. MSSQL(`tiberius`)은 이후, Oracle 등은 JDBC 브리지 확장 검토 |
| DB 그리드 | Glide Data Grid (canvas) | 수십만 행 가상 스크롤 |
| SQL 편집 | CodeMirror `@codemirror/lang-sql` + 스키마 기반 자동완성 | |
| 디버깅 | **DAP 클라이언트** (Rust 구현) | js-debug, debugpy, codelldb, delve, java-debug |
| 코드 인텔리전스 | **LSP 클라이언트** (`async-lsp` / `lsp-types`) | 서버 자동 설치 관리자 포함 |
| 파일 검색 | `ignore` + `frizbee`(퍼지, MIT) | .gitignore 존중. `nucleo` 는 MPL-2.0 이라 쓰지 않는다(아래 「라이선스 검사」 행) |
| 텍스트 검색 | `grep-searcher`/`grep-regex` (ripgrep 라이브러리) | |
| 심볼 인덱스 | tree-sitter tags → SQLite(FTS5) | LSP 미기동/미지원 언어 폴백 |
| 파일 감시 | `notify` (+ debounce) | |
| 파일 위치 | **XDG Base Directory** (`etcetera` 크레이트, macOS도 XDG) | 03 §5-0 |
| 언어 판별 | **GitHub Linguist** 데이터(`languages.yml` · `heuristics.yml` 등) → 빌드 시 내장 테이블 | 07 §4.3 |
| LSP/DAP 설치 정보 | **Mason registry** (`package.yaml`, purl 기반) + 자체 설치기 | 07 §4.4 |
| 런타임 환경 | **mise** 연동(CLI 호출 · JSON 출력) 우선, direnv/asdf/로그인 셸 대체 | 필수 의존 아님 (03 §5-1) |
| 저장소 개발 도구 버전 | `mise.toml` (Rust · Node · pnpm 고정) | 워크트리 · CI · 에이전트 간 동일 버전 |
| 로컬 저장소 | SQLite (`rusqlite`) + TOML 설정 파일 | |
| 비밀 | `keyring` (macOS Keychain / Windows Credential / Secret Service) | |
| 원격 | **Pitwall Relay 데몬** + 연결 계층 `pw-link` (QUIC `quinn`, Noise `snow`, mDNS) | 직접 연결(LAN/Tailscale) → SSH(`russh`) 대체. 중계 서버 없음 (08 참고) |
| 모바일 | (보류) React Native (Expo) 후보 | D13 — 로드맵 제외 |
| 라이선스 검사 | `cargo-deny`, `pnpm licenses list`(npm, `scripts/licenses.mjs`) | MIT 프로젝트 — 허용형 라이선스만. MPL-2.0 은 Tauri 전이 의존성에 한해 크레이트 단위 예외(수정 없이 사용, `deny.toml` `[[licenses.exceptions]]`). CC0-1.0(퍼블릭 도메인 헌정, 의무 없음)은 파일 감시 `notify` 에 한해 같은 방식의 크레이트 단위 예외. BSL-1.0(Boost, 허용형 · 바이너리 배포에 고지 의무 없음)은 클립보드 플러그인의 Windows 전용 전이 의존성(`clipboard-win` · `error-code`)에 한해 같은 방식의 크레이트 단위 예외 — 전역 허용 목록에는 넣지 않는다. CDLA-Permissive-2.0(데이터용 허용형)은 자동 업데이트(`tauri-plugin-updater` → `reqwest` → `rustls-platform-verifier`)가 끌어오는 `webpki-root-certs` 에 한해 같은 방식의 크레이트 단위 예외 — wasm32 전용 의존성이라 배포하는 3 OS 바이너리에 들어가지 않는다(`cargo tree -e normal -i webpki-root-certs --target <배포 타깃>` 이 비어 있다). 배포 타깃의 그래프에 들어오면 예외를 걷고 다시 판단한다. npm 도 dev 전용 패키지에 한해 패키지 단위 예외(이유 필수, prod 에 나타나면 실패, `licenses/npm-exceptions.toml`) |
| 확장 런타임 | `wasmtime` (WASM Component Model, WIT) | 권한 기반 샌드박스 (07 참고) |
| 테마 | 디자인 토큰 → CSS 변수, VS Code 테마 import | |
| UI 언어(i18n) | **Fluent** (`fluent-rs` / `@fluent/bundle`) | 코어 · 데스크톱 공용 리소스 |
| 문법 | tree-sitter 문법을 **WASM으로 동적 로드** | 언어 추가를 확장으로 |
| RPC | 자체 JSON-RPC 2.0 (serde) over Tauri IPC / stdio / WebSocket | 타입은 `ts-rs`/`specta`로 TS 생성 |
| MCP | **`rmcp`** (공식 Rust MCP SDK) | stdio + Streamable HTTP |
| 패키징 | Tauri bundler (dmg/msi/AppImage/deb), 자동 업데이트 plugin-updater | |
| 단일 인스턴스 | tauri-plugin-single-instance + deep-link | 파일 열기 → 기존 인스턴스로 전달 |
| 테스트 | Rust: cargo-nextest, insta / UI: Vitest, Playwright | |

## 2. 주요 결정과 근거

### 2.1 Tauri vs Electron vs 네이티브(GPUI 등)

| | Tauri 2 | Electron | GPUI(Zed) / 네이티브 |
|---|---|---|---|
| 메모리/번들 | ◎ (수십 MB) | △ (Chromium 포함) | ◎ |
| UI 생산성 | ◎ (웹) | ◎ (웹) | △ (생태계 작음, 직접 구현 많음) |
| 렌더링 일관성 | △ (OS WebView 차이, 특히 Linux WebKitGTK) | ◎ | ◎ |
| 시스템 작업(git/pty/ssh) | ◎ Rust | ○ Node 네이티브 모듈 | ◎ |

- "IntelliJ보다 가볍게"가 핵심 목표 → Electron 제외
- GPUI는 성능은 최상이지만 DB 그리드 · git 그래프 · 폼 UI 등 구현량이 너무 큼
- **리스크**: Linux WebKitGTK 성능. 대응 → 무거운 뷰(그래프, 그리드)는 canvas 렌더링, 가상화 필수.
  최악의 경우 코어가 헤드리스이므로 UI만 Electron으로 교체 가능 (아키텍처상 보험).

### 2.2 에디터: CodeMirror 6 vs Monaco
- Monaco는 VS Code 수준 기능이지만 무겁고(수 MB, 워커 다수) 다중 인스턴스에 약함
- 편집 수준은 **L2 + 멀티 커서**로 결정 (D7) — CodeMirror 6이 자동완성 · 멀티 커서(`allowMultipleSelections`, 사각 선택)를 기본 지원하므로 충분 → CodeMirror 6
- LSP 기능(hover, go-to-def, diagnostics, completion)은 코어의 LSP 클라이언트 결과를 CodeMirror 확장으로 연결

### 2.3 Git: gitoxide + git CLI 하이브리드
- 그래프 · 로그 필터 · blame · diff는 대량 데이터 → in-process `gix`로 빠르게, 스트리밍
- 쓰기 작업은 사용자 환경 그대로 동작해야 함 (pre-commit 훅, GPG/SSH 서명, credential helper, LFS)
  → `git` CLI 호출이 가장 안전. `libgit2`는 훅/서명 미지원 문제가 있어 제외
- 필요 시스템 요구사항: git ≥ 2.40

### 2.4 코드 인텔리전스: LSP + tree-sitter, 자체 인덱서 없음
- IntelliJ PSI 수준 인덱서는 비목표
- 1차: 언어별 LSP (rust-analyzer, tsserver/vtsls, pyright/basedpyright, gopls, jdtls, kotlin-lsp 등)
- 2차(폴백/즉시성): tree-sitter tags로 만든 심볼 인덱스 — LSP 기동 전에도 "심볼로 이동" 가능
- LSP · DAP · 포매터 설치 정보는 **Mason registry**를 데이터로 사용, 설치는 자체 설치기 → `$DATA/tools/` (07 §4.4)
- 파일 언어 판별은 **GitHub Linguist** 데이터 (07 §4.3)

### 2.5 디버깅: DAP
- 언어별 디버거를 직접 구현하지 않고 Debug Adapter Protocol만 구현
- 기본 번들/자동 설치 대상: `js-debug`(Node/브라우저), `debugpy`, `codelldb`(Rust/C/C++), `delve`(Go), `java-debug`(jdtls 확장)

### 2.6 RPC 계약 우선
- 코어 API는 Rust 타입으로 정의 → `specta`로 TS 타입 생성, MCP 툴 스키마도 같은 타입에서 `schemars`로 생성
- 하나의 정의에서 UI · MCP · 원격 프로토콜이 파생됨 → 불일치 방지

### 2.7 DB 계층: DBX 구조 참고

[DBX](https://github.com/t8y2/dbx)(Apache-2.0, Tauri 2 + Vue 3 + CodeMirror 6, Rust `sqlx`/`tiberius`/`redis-rs`)는 Pitwall과 스택이 거의 같고 성숙한(2026-10 기준 스타 2.5만) DB 클라이언트다. Apache-2.0은 MIT 프로젝트에서 차용 가능(라이선스 · NOTICE 고지, 변경 표기 필요).

DBX의 크레이트 분할을 참고해 `pw-db`를 다음처럼 나눈다:

| Pitwall | 참고한 DBX 크레이트 | 역할 |
|---|---|---|
| `pw-db-types` | `dbx-types` | 연결 · 쿼리 · 메타데이터 공용 타입 |
| `pw-db-sql` | `dbx-sql` | SQL 파싱(sqlparser-rs), 방언, **위험도 분류**, DDL/DML 생성 |
| `pw-db-driver` | `dbx-drivers`, `dbx-driver-support` | 드라이버 trait, 생명주기, 실행 제한(행 수 · 타임아웃), SSH 터널 |
| `pw-db-driver-{postgres,mysql,sqlite,redis}` | `dbx-driver-*` | 드라이버별 구현 (드라이버마다 크레이트 분리 → 빌드 · 의존성 격리) |
| `pw-db` | `dbx-core` | 연결 풀, 쿼리 실행/스트리밍, 스키마 캐시, 히스토리 |
| (P2) JDBC 브리지 확장 | `dbx-driver-agent`, `agents/` | 별도 JVM 프로세스 + 드라이버 jar 설치 |

추가로 참고할 점:
- SQLite 파일은 **격리된 워커 프로세스**에서 연다(`dbx-sqlite-worker`) → 손상 파일/긴 쿼리가 메인 프로세스를 막지 않음
- DBX도 MCP 서버(`dbx-mcp`)와 CLI를 같은 코어 위에 제공 → Pitwall의 헤드리스 Host 원칙과 일치
- 구현 착수(M3) 전에 드라이버 trait · 위험도 분류 · Redis 뷰어 코드를 검토하고, 차용 시 `THIRD_PARTY_NOTICES.md`에 기록

## 3. 지원 플랫폼 — macOS 우선

| 플랫폼 | 등급 | 내용 |
|---|---|---|
| **macOS** (Apple Silicon 우선, x64) | **Tier 1** | 개발 · QA 기준 플랫폼. 서명 · 공증(notarization), Homebrew cask(M0 이후), 유니버설 바이너리 |
| Linux (x64/arm64, AppImage/deb/rpm) | Tier 2 | CI 빌드 + 스모크 테스트, WebKitGTK 성능 검증 |
| Windows (x64/arm64, msi) | Tier 2 | CI 빌드 + 스모크 테스트, WebView2 |
| Relay (원격) | Linux x64/arm64(musl 정적) Tier 1, macOS Tier 1, Windows Tier 2 | 원격 서버는 대부분 Linux이므로 Relay는 Linux도 1급 |
| 모바일 | 보류 | |

macOS 우선 항목:
- 네이티브 메뉴바, `⌘` 단축키 체계, 전체화면/탭 창, Keychain
- Finder 연동: "Pitwall로 열기" 빠른 동작(Quick Action), Dock 아이콘에 파일 드롭, `open -a Pitwall <path>`
- 외부 도구 감지: `/Applications`, JetBrains Toolbox 경로, `mdfind`로 번들 ID 검색
- 플랫폼 의존 코드는 `pw-platform` 크레이트에 격리 → 다른 OS 지원 시 구현만 추가

## 3-1. 배포 · 업데이트 (D21)

| 항목 | 결정 |
|---|---|
| 배포 위치 | **GitHub Releases** (설치 파일 + 그 릴리스의 업데이트 매니페스트 `latest.json`) |
| 자동 업데이트 | Tauri `plugin-updater` (서명된 업데이트 패키지 검증). 서명에 실린 버전이 매니페스트의 버전과 다르면 거절한다(`requireSignedVersion` — 옛 릴리스의 서명된 패키지에 높은 버전을 붙인 매니페스트로 되돌리지 못하게). 시작할 때 조용히 확인(새 버전이 있을 때만 묻는다) + 메뉴 「업데이트 확인…」. 시작 확인은 release 빌드가 기본 `$DATA` 로 떴을 때만 한다 — 개발 · 측정 · 시험 실행은 네트워크에 나가지 않는다 |
| 채널 | **stable / preview** 2개 — preview는 태그 `vX.Y.Z-preview.N`, 설정에서 선택(전역 `settings.toml` 의 `[update] channel`, 내장 기본값 `"stable"`) |
| 매니페스트 위치 | 릴리스마다 **그 릴리스의 자산**으로 `latest.json` 하나. stable 앱은 `releases/latest/download/latest.json`(GitHub 가 prerelease · draft 를 건너뛴다), preview 앱은 GitHub REST API 의 릴리스 목록에서 `latest.json` 을 가진 가장 높은 버전(stable 포함)의 것을 읽는다. 이유: preview 릴리스가 stable 매니페스트를 덮어쓸 자리가 구조적으로 없고, gh-pages 브랜치나 상설 릴리스 같은 저장소 인프라를 새로 두지 않는다. 대가: preview 확인은 GitHub API 의 인증 없는 한도(IP 당 시간당 60회)를 쓴다 — 넘으면 그 확인만 실패한다 |
| 버전 원천 | **소스**(워크스페이스 `Cargo.toml` 의 `version`). 태그는 그 커밋의 소스 버전과 같아야 하고 다르면 워크플로가 실패한다. 이유: 태그에서 버전을 써 넣으면 `CARGO_PKG_VERSION`(로그 · 확장 엔진 버전 · 진단)과 번들 버전이 갈리거나 CI 가 소스를 고쳐야 한다. 같은 커밋을 로컬에서 빌드해도 같은 버전이 나온다. 대가: 릴리스마다 버전을 올리는 커밋이 하나 필요하다 |
| macOS | **Apple Developer 계정**(연 $99)으로 서명 · 공증, **유니버설** dmg + updater 패키지(`.app.tar.gz`, 두 아키텍처 키가 함께 쓴다), Homebrew cask 제공(M0 이후) |
| Windows | 코드 서명은 출시 시점에 결정 (초기엔 미서명 + SmartScreen 안내). msi 만 낸다. MSI 버전은 prerelease 를 숫자로만 받아 preview 의 N 을 넷째 칸에 둔다(`0.1.0-preview.2` → `0.1.0.2`) |
| Linux | AppImage(자동 업데이트) / deb / rpm. **preview 는 AppImage 만, deb · rpm 은 stable 만** — 이유: 패키지 관리자(dpkg · rpm)가 `X.Y.Z-preview.N` 을 `X.Y.Z` 보다 높게 정렬해 preview 패키지에서 같은 X.Y.Z 의 stable 로 올라가지 못한다(rpm 은 Version 에 `-` 도 받지 않는다). preview 를 쓰는 Linux 사용자는 자동 업데이트되는 AppImage 를 쓴다. 매니페스트 키는 `linux-x86_64-appimage` 다 — 맨 `linux-x86_64` 를 쓰면 deb · rpm 설치본이 AppImage 를 받아 설치하려 든다. deb · rpm 설치본의 「업데이트 확인…」 은 패키지 관리자 안내를 띄운다 |
| Relay | Desktop과 같은 릴리스에 musl 정적 바이너리 포함 (버전 정확히 일치, 08) |
| 빌드 | GitHub Actions(`.github/workflows/release.yml`): 태그 푸시 → plan(태그 = 소스 버전, 태그 커밋이 main 의 조상) → 3개 OS 빌드 · 서명 · 공증 → updater 재서명(운영 키) → 매니페스트 → draft Release 에 자산을 다 올린 뒤 공개. 그 밖의 트리거(릴리스 파이프라인을 바꾼 PR, `workflow_dispatch`)는 **dry-run** — 같은 빌드와 Apple 서명 · 공증을 하고, 아티팩트로는 체크섬 · 검증 로그 · `latest.json` 같은 글만 남긴다(공개 저장소의 아티팩트는 누구나 받을 수 있어 서명된 바이너리를 올리지 않는다). 태그를 골라 `workflow_dispatch` 해도 dry-run 이다(공개 판정은 이벤트까지 본다). updater 서명은 build 잡이 늘 일회용 키로 하고, 공개 실행에서만 별도 잡(`sign-updater`)이 운영 키로 다시 서명한다 — 운영 updater 키 Secret 을 참조하는 잡은 그 하나뿐이고 그 잡은 저장소 코드를 돌리지 않는다. 이유: Secret 을 참조하는 잡의 runner 에는 값이 내려가 같은 잡의 PR 코드가 읽어 낼 수 있고, updater 키는 바꿀 수 없다. GitHub Environment 로 가두는 방법은 Secret 을 사람이 다시 넣어야 해서 쓰지 않았다. Release 를 쓰는 권한은 tag push 에서만 도는 잡에만 있다. 절차는 `CONTRIBUTING.md` 「릴리스」 |

## 4. 레포 구조 (예정)

```
pitwall/
├─ apps/
│  ├─ desktop/           # Tauri 앱 (src-tauri + React UI)
│  ├─ relay/             # pitwall-relay: 헤드리스 코어 데몬 (원격 설치용 겸 MCP 서버)
│  └─ (mobile/)          # 보류
├─ crates/
│  ├─ pw-core/           # 프로젝트/워크스페이스, 이벤트 버스, 설정
│  ├─ pw-rpc/            # RPC 타입/프로토콜, transport(IPC/stdio/ws)
│  ├─ pw-rpc-gen/        # RPC 선언 → TS 타입 · JSON Schema 생성기 (`mise run gen`, 출력은 packages/rpc-client)
│  ├─ pw-diag/           # 로그(tracing, 구성요소별 회전 파일) · 진단 정보 · 패닉 훅 (03 「로그 · 진단 (D22)」)
│  ├─ pw-perf/           # 콜드 스타트 · 유휴 메모리 측정 하네스 (`mise run perf`, 06 「성능 목표 (검증 기준)」)
│  ├─ pw-fs/             # 파일 트리, 감시, 검색
│  ├─ pw-git/            # gix 읽기 + CLI 쓰기
│  ├─ pw-db*/            # pw-db, pw-db-types, pw-db-sql, pw-db-driver(-postgres/-mysql/-sqlite/-redis) (§2.7)
│  ├─ pw-lsp/            # LSP 클라이언트, 서버 관리자
│  ├─ pw-dap/            # DAP 클라이언트
│  ├─ pw-runtime/        # 런타임 제공자 (mise · direnv · asdf · 로그인 셸), env 캐시 — 지금은 로그인 셸 PATH 만(03 §5-1)
│  ├─ pw-open-with/      # 외부 도구로 열기: 도구 목록(확장 + 전역 설정) · 기본 도구 · 셸 없는 실행 (04 F8)
│  ├─ pw-run/            # 실행 구성, 프로세스/PTY 관리, 태스크 감지
│  ├─ pw-lang/           # 언어 판별 (Linguist 데이터 테이블, .gitattributes, 모드라인)
│  ├─ pw-tools/          # Mason registry 읽기 · 도구 설치기 (LSP/DAP/포매터)
│  ├─ pw-index/          # tree-sitter 심볼 인덱스
│  ├─ pw-link/           # 기기 키 · 페어링 · Noise 세션 · 전송(QUIC/SSH) · Tailscale 검색
│  ├─ pw-relay/          # Relay 데몬, 기기별 권한, SSH 설치 부트스트랩
│  ├─ pw-ext/            # 확장 로더, 매니페스트, wasmtime 호스트, 기여 레지스트리
│  ├─ pw-i18n/           # Fluent 리소스 로딩
│  ├─ pw-platform/       # OS별 통합 (macOS 우선)
│  └─ pw-mcp/            # MCP 서버 (rmcp)
├─ packages/
│  ├─ ui/                # React 컴포넌트
│  ├─ rpc-client/        # 생성된 TS 타입 + 클라이언트 (데스크톱 · MCP 등 클라이언트 공용)
│  ├─ domain/            # 공용 상태/포맷 로직 (데스크톱 · MCP 등 클라이언트 공용)
│  ├─ logger/            # 웹뷰 로그를 묶어 코어의 `log.write` 로 보내는 로거
│  └─ ext-api/           # L3 UI 패널 확장용 TS API
├─ extensions/           # 내장 확장 (기본 테마, 언어, 외부 도구, 언어팩)
└─ docs/
```
- Rust: cargo workspace / TS: pnpm workspace
