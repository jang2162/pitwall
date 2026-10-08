# 02. 기술 스택

## 1. 결론

| 영역 | 선택 | 비고 |
|---|---|---|
| 앱 셸 | **Tauri 2** | 네이티브 WebView, 작은 번들 · 낮은 메모리 |
| 코어 언어 | **Rust** (tokio) | git/DB/LSP/DAP/PTY/SSH 생태계가 모두 Rust에 존재 |
| UI | **React 19 + TypeScript + Vite** | 가상화 리스트 · 그리드 · 에디터 생태계 |
| UI 상태 | Zustand + TanStack Query | 코어 이벤트 스트림 → 쿼리 캐시 무효화 |
| UI 컴포넌트 | Radix UI primitives + Tailwind | 키보드 접근성, IDE형 밀도 높은 레이아웃 |
| 레이아웃(도킹) | dockview | 탭/스플릿/도킹 패널 |
| 에디터 | **CodeMirror 6** | 경량, 뷰어 우선. Monaco 대비 번들 · 메모리 작음 |
| 구문 강조 | 코어: **tree-sitter** / UI: Lezer(CodeMirror) + tree-sitter 하이라이트 결과 오버레이 | 언어 범위는 tree-sitter 기준 |
| Diff 뷰 | @codemirror/merge | side-by-side, inline |
| 터미널 | xterm.js + `portable-pty` | 실행 콘솔/터미널 공용 |
| Git 읽기 | **gitoxide (`gix`)** | log/graph/status/blame/diff를 빠르게 |
| Git 쓰기 | **git CLI** | commit/merge/rebase/checkout/push — 훅, 서명, credential helper 존중 |
| DB | **sqlx** (Postgres/MySQL/SQLite) + **`redis-rs`** (Redis) | 드라이버 추상화 계층 위에 얹음. MSSQL(`tiberius`)은 이후, Oracle 등은 JDBC 브리지 확장 검토 |
| DB 그리드 | Glide Data Grid (canvas) | 수십만 행 가상 스크롤 |
| SQL 편집 | CodeMirror `@codemirror/lang-sql` + 스키마 기반 자동완성 | |
| 디버깅 | **DAP 클라이언트** (Rust 구현) | js-debug, debugpy, codelldb, delve, java-debug |
| 코드 인텔리전스 | **LSP 클라이언트** (`async-lsp` / `lsp-types`) | 서버 자동 설치 관리자 포함 |
| 파일 검색 | `ignore` + `nucleo`(퍼지) | .gitignore 존중 |
| 텍스트 검색 | `grep-searcher`/`grep-regex` (ripgrep 라이브러리) | |
| 심볼 인덱스 | tree-sitter tags → SQLite(FTS5) | LSP 미기동/미지원 언어 폴백 |
| 파일 감시 | `notify` (+ debounce) | |
| 로컬 저장소 | SQLite (`rusqlite`) + TOML 설정 파일 | |
| 비밀 | `keyring` (macOS Keychain / Windows Credential / Secret Service) | |
| 원격 | **Pitwall Relay 데몬** + 연결 계층 `pw-link` (QUIC `quinn`, Noise `snow`, mDNS) | 직접 연결(LAN/Tailscale) → SSH(`russh`) 대체. 중계 서버 없음 (08 참고) |
| 모바일 | **React Native (Expo)** + `uniffi`로 `pw-link` Kotlin 바인딩 | **Android 먼저**, 포그라운드 서비스로 상시 연결 |
| 라이선스 검사 | `cargo-deny`, `license-checker`(npm) | MIT 프로젝트 — 허용형 라이선스만 |
| 확장 런타임 | `wasmtime` (WASM Component Model, WIT) | 권한 기반 샌드박스 (07 참고) |
| 테마 | 디자인 토큰 → CSS 변수, VS Code 테마 import | |
| UI 언어(i18n) | **Fluent** (`fluent-rs` / `@fluent/bundle`) | 코어 · 데스크톱 · 모바일 공용 리소스 |
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
- LSP 서버 바이너리는 자동 다운로드 관리 (Zed/Mason 방식, `~/.pitwall/servers`)

### 2.5 디버깅: DAP
- 언어별 디버거를 직접 구현하지 않고 Debug Adapter Protocol만 구현
- 기본 번들/자동 설치 대상: `js-debug`(Node/브라우저), `debugpy`, `codelldb`(Rust/C/C++), `delve`(Go), `java-debug`(jdtls 확장)

### 2.6 RPC 계약 우선
- 코어 API는 Rust 타입으로 정의 → `specta`로 TS 타입 생성, MCP 툴 스키마도 같은 타입에서 `schemars`로 생성
- 하나의 정의에서 UI · MCP · 원격 프로토콜이 파생됨 → 불일치 방지

## 3. 지원 플랫폼 — macOS 우선

| 플랫폼 | 등급 | 내용 |
|---|---|---|
| **macOS** (Apple Silicon 우선, x64) | **Tier 1** | 개발 · QA 기준 플랫폼. 서명 · 공증(notarization), Homebrew cask, 유니버설 바이너리 |
| Linux (x64/arm64, AppImage/deb/rpm) | Tier 2 | CI 빌드 + 스모크 테스트, WebKitGTK 성능 검증 |
| Windows (x64/arm64, msi) | Tier 2 | CI 빌드 + 스모크 테스트, WebView2 |
| Relay (원격) | Linux x64/arm64(musl 정적) Tier 1, macOS Tier 1, Windows Tier 2 | 원격 서버는 대부분 Linux이므로 Relay는 Linux도 1급 |
| 모바일 | **Android Tier 1**, iOS 이후 | |

macOS 우선 항목:
- 네이티브 메뉴바, `⌘` 단축키 체계, 전체화면/탭 창, Keychain
- Finder 연동: "Pitwall로 열기" 빠른 동작(Quick Action), Dock 아이콘에 파일 드롭, `open -a Pitwall <path>`
- 외부 도구 감지: `/Applications`, JetBrains Toolbox 경로, `mdfind`로 번들 ID 검색
- 플랫폼 의존 코드는 `pw-platform` 크레이트에 격리 → 다른 OS 지원 시 구현만 추가

## 4. 레포 구조 (예정)

```
pitwall/
├─ apps/
│  ├─ desktop/           # Tauri 앱 (src-tauri + React UI)
│  ├─ relay/             # pitwall-relay: 헤드리스 코어 데몬 (원격 설치용 겸 MCP 서버)
│  └─ mobile/            # Expo 앱 (Android 우선)
├─ crates/
│  ├─ pw-core/           # 프로젝트/워크스페이스, 이벤트 버스, 설정
│  ├─ pw-rpc/            # RPC 타입/프로토콜, transport(IPC/stdio/ws)
│  ├─ pw-fs/             # 파일 트리, 감시, 검색
│  ├─ pw-git/            # gix 읽기 + CLI 쓰기
│  ├─ pw-db/             # DB 드라이버 추상화, 쿼리 실행, 스키마 인트로스펙션
│  ├─ pw-lsp/            # LSP 클라이언트, 서버 관리자
│  ├─ pw-dap/            # DAP 클라이언트
│  ├─ pw-run/            # 실행 구성, 프로세스/PTY 관리, 태스크 감지
│  ├─ pw-index/          # tree-sitter 심볼 인덱스
│  ├─ pw-link/           # 기기 키 · 페어링 · Noise 세션 · 전송(QUIC/SSH) · Tailscale 검색
│  ├─ pw-relay/          # Relay 데몬, 기기별 권한, SSH 설치 부트스트랩
│  ├─ pw-ext/            # 확장 로더, 매니페스트, wasmtime 호스트, 기여 레지스트리
│  ├─ pw-i18n/           # Fluent 리소스 로딩
│  ├─ pw-platform/       # OS별 통합 (macOS 우선)
│  └─ pw-mcp/            # MCP 서버 (rmcp)
├─ packages/
│  ├─ ui/                # React 컴포넌트
│  ├─ rpc-client/        # 생성된 TS 타입 + 클라이언트 (데스크톱 · 모바일 공용)
│  ├─ domain/            # 공용 상태/포맷 로직 (데스크톱 · 모바일 공용)
│  └─ ext-api/           # L3 UI 패널 확장용 TS API
├─ extensions/           # 내장 확장 (기본 테마, 언어, 외부 도구, 언어팩)
└─ docs/
```
- Rust: cargo workspace / TS: pnpm workspace
