# 11. M0 (기반) 작업 분할

M0 목표: **기능은 없지만 이후 모든 기능이 올라갈 뼈대가 동작하는 상태.**
완료 시 할 수 있는 것: 앱 실행 → 웰컴 창 → 폴더를 프로젝트로 등록 → 프로젝트 창(빈 레이아웃) 열기, `pitwall <path>`로 기존 창에 전달, 테마 · 언어 전환, 단축키로 액션 실행, `pitwall mcp`로 `project_list` 호출.

대상 플랫폼: macOS(개발 · 검증), Linux/Windows는 CI 빌드만.

## 1. 작업 목록

| ID | 작업 | 내용 | 선행 | 완료 기준 |
|---|---|---|---|---|
| **M0-01** | 저장소 골격 | cargo workspace + pnpm workspace, 02 §4 디렉터리 구조, rustfmt/clippy/eslint/prettier, `rust-toolchain.toml`, `.editorconfig`, **`mise.toml`(Rust · Node · pnpm 버전 고정 + `build`/`test`/`lint` 태스크)** | — | 새 워크트리에서 `mise install && mise run test` 통과 |
| **M0-02** | 개발 규칙 문서 | `CLAUDE.md`/`AGENTS.md`(에이전트용 규칙: 구조 · 명령 · 테스트 · 커밋 규칙), `CONTRIBUTING.md`, 커밋 메시지 규칙 | M0-01 | 에이전트가 문서만 보고 빌드 · 테스트 실행 가능 |
| **M0-03** | CI | GitHub Actions: `mise` 액션으로 같은 도구 버전 설치 → lint · test (3 OS), `cargo-deny`(라이선스 · 취약점), `license-checker`(npm), `THIRD_PARTY_NOTICES.md` 생성 | M0-01 | PR마다 그린 |
| **M0-04** | Tauri 셸 | Tauri 2 앱, macOS overlay 타이틀바, 다중 창(웰컴 / 프로젝트 / 설정), 웹뷰 기본 동작 차단(`⌘R`, 줌, 뒤로가기) | M0-01 | 빈 창 3종이 뜸 |
| **M0-05** | RPC 계약 | `pw-rpc`: JSON-RPC 2.0, 메서드 정의 매크로(이름 · 입력/출력 타입 · **role 메타데이터** · 권한 등급), 스트림(`streamId` + credit), 이벤트 구독, 취소 | M0-01 | 단위 테스트: 요청/응답 · 스트림 · 취소 · 권한 거부 |
| **M0-06** | 타입 생성 | `specta`로 TS 타입 + `rpc-client` 생성, `schemars`로 JSON Schema 생성 (MCP · 폼용), CI에서 생성물 최신 여부 검사 | M0-05 | TS에서 타입 안전하게 호출 |
| **M0-07** | Host 추상 · 이벤트 버스 | `pw-core`: `Host` trait, `LocalHost`(in-process), `ProjectContext`, 이벤트 버스, `Task`(진행률 · 취소), Host Router(project_id → Host) | M0-05 | Tauri IPC transport로 UI ↔ LocalHost 왕복 |
| **M0-08** | 설정 계층 | TOML 로딩/병합(내장 → `.pitwall/*.toml` → `*.local.toml`, 워크트리면 메인 저장소 `.local` 폴백), **`schema` 버전 · 마이그레이션 프레임워크 · 백업**, 파일 감시로 재로드, 개인 전용 모드 | M0-07 | 마이그레이션 v0→v1 샘플 테스트 |
| **M0-09** | 프로젝트 레지스트리 | `state.db`(SQLite), 프로젝트 CRUD, 최근 목록, 경로 → 프로젝트 해석(가장 깊은 루트, **git worktree → 메인 저장소**, F2-5), 루트 후보 추천(.git · 빌드 파일) | M0-07, M0-08 | 해석 규칙 단위 테스트 (중첩 · 워크트리 · 심링크) |
| **M0-10** | CLI · 단일 인스턴스 | `pitwall <path>` (디렉터리 · 파일 · `path:line:col`), single-instance로 기존 앱에 전달, "Shell 명령 설치"(macOS `/usr/local/bin`), `pitwall mcp` 서브커맨드 진입점 | M0-04, M0-09 | Orca "Open in"과 같은 방식(`spawn("pitwall", [absPath])`)으로 호출 시 올바른 창 활성화 |
| **M0-11** | UI 기반 | React 19 + Vite + Tailwind v4 + Radix, 디자인 토큰 → CSS 변수, 라이트/다크(OS 연동), dockview 기본 레이아웃(툴 윈도우 바 · 에디터 영역 · 하단 · 상태바 자리), TanStack Query + 이벤트 무효화 | M0-04, M0-06 | 레이아웃 저장/복원, 테마 전환 |
| **M0-12** | 웰컴 창 · 프로젝트 찾기 | S1(목록 · 새 프로젝트 · 폴더 열기 · 드롭), S2(루트 후보 팝업), S3(생성 시트 — 자동 감지 결과는 빈 목록) | M0-09, M0-11 | 10-ui §2.1~2.3 화면 동작 |
| **M0-13** | 키맵 엔진 · 액션 | `packages/keymap`: 액션 레지스트리, `when` 컨텍스트, 더블 탭, `KeyboardEvent.code` 매칭, IME 조합 중 무시, 기본 키맵 파일 + `keymap.toml` 덮어쓰기, 네이티브 메뉴 accelerator 동기화, 액션 찾기(`⌘⇧A`, cmdk) | M0-11 | 한글 입력 상태에서 `⌘⇧A` 동작, 덮어쓰기 반영 |
| **M0-14** | i18n | Fluent 리소스(ko/en), Rust · TS 공용 로딩, 언어 전환 | M0-11 | 웰컴 창 전체 번역 |
| **M0-15** | 확장 레지스트리 골격 | `pw-ext`: `extension.toml` 파서, 기여 레지스트리(테마 · 키맵 · 언어 · 외부 도구 · UI 번역), **내장 확장**으로 기본 테마 · 기본 키맵 · 외부 도구 목록 등록 (WASM 런타임은 이후) | M0-08 | 기본 테마/키맵이 확장 경로로 로드됨 |
| **M0-16** | 외부 도구로 열기 (최소) | 외부 도구 레지스트리 + 명령 템플릿 실행(Orca · Zed · IntelliJ · VS Code · Finder · 터미널), `⌥F1` 팝업, `⌘⌥E` | M0-13, M0-15 | 각 도구로 프로젝트 열기 (F8 P0 일부 선행) |
| **M0-17** | MCP 골격 | `pw-mcp`(rmcp): stdio(`pitwall mcp` → 실행 중 앱에 로컬 소켓 연결), 툴 레지스트리(RPC 메서드에서 생성), 툴별 on/off · 프리셋, 감사 로그, `project_list` · `project_info` 툴 | M0-05, M0-10 | Claude Code에서 `project_list` 호출 성공 |
| **M0-18** | 로그 · 진단 | 회전 로그(코어/UI), "진단 정보 복사", 크래시 리포트 설정 자리(전송은 이후) | M0-07 | 로그 파일 생성 · 진단 복사 |
| **M0-19** | 릴리스 파이프라인 | 태그 → 3 OS 빌드, macOS 서명 · 공증, updater 매니페스트, stable/preview 채널 | M0-03, M0-04 | preview 릴리스 1회 배포 · 자동 업데이트 확인 |
| **M0-20** | 성능 기준선 | 콜드 스타트 · 유휴 메모리 측정 스크립트, CI에 기록 | M0-12 | 기준선 수치 기록 (목표: 1.5s / 300MB) |

## 2. 순서 (의존 관계)

```
M0-01 ─┬─ M0-02
       ├─ M0-03 ───────────────────────────────┐
       ├─ M0-04 ─────────────┬─ M0-11 ─┬─ M0-12  │
       └─ M0-05 ─┬─ M0-06 ───┘         ├─ M0-13 ─┼─ M0-16
                 └─ M0-07 ─┬─ M0-08 ─┬─ M0-09 ─┴─ M0-10 ── M0-17
                           │         └─ M0-15
                           └─ M0-18                M0-19 (M0-03, M0-04 이후)
                                                   M0-20 (M0-12 이후)
```

- 병렬 가능 묶음: (M0-02, M0-03, M0-04, M0-05) → (M0-06, M0-07) → (M0-08, M0-11, M0-18) → 나머지
- 에이전트 병렬 작업 시 워크트리 단위로 나누기 좋은 경계: Rust 코어(05·07·08·09) / UI(11·12·13·14) / 인프라(03·19·20)

## 3. M0에서 하지 않는 것
- 파일 트리 · 에디터 · git · DB 등 실제 기능 (M1~)
- WASM 플러그인 런타임, Relay, 원격 연결
- 키맵 커스텀 UI (파일 편집만 동작)

## 4. 착수 전 준비물
- 개발 머신에 mise 설치 (도구 버전은 `mise.toml`이 관리)
- Apple Developer 계정 (M0-19 서명 · 공증)
- GitHub 저장소 Actions · Releases 권한, updater 서명 키 생성 · 보관(Secrets)
