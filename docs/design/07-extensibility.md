# 07. 확장성 (테마 · 언어 · 플러그인)

v1부터 마켓플레이스를 열지는 않지만, **내장 기능도 확장 API 위에서 구현**해서 나중에 외부 확장을 열 때 구조를 바꾸지 않게 한다.
(예: 기본 테마, 기본 언어 지원, 기본 DB 드라이버, 기본 외부 도구 목록을 모두 "내장 확장"으로 등록)

## 1. 확장 단위와 계층

| 계층 | 형태 | 할 수 있는 일 | 샌드박스 | 도입 |
|---|---|---|---|---|
| **L1. 선언형 기여** | `extension.toml` + 정적 파일 | 테마, UI 번역, 아이콘 팩, 키맵(베이스 키맵 · 바인딩 묶음), 코드 언어(문법·LSP·DAP 정의), 실행 구성 템플릿, 태스크 감지 규칙, 외부 도구, 스니펫 | 실행 코드 없음 | v1.0 |
| **L2. WASM 플러그인** | WebAssembly 컴포넌트 (WIT 인터페이스) | 코어 로직 확장: 커스텀 태스크 감지, 실행 구성 타입, LSP/DAP 바이너리 설치 로직, 커맨드, MCP 툴 추가 | wasmtime + 권한(capability) 기반 | v1.x |
| **L3. UI 패널** | 격리 iframe 웹뷰 | 커스텀 패널/뷰 (예: Kafka 뷰어, API 클라이언트) | 별도 origin + postMessage API, CSP | v2 |
| **L4. 네이티브 드라이버** | Rust 크레이트 (앱에 빌드 포함) | DB 드라이버, VCS 백엔드 | 없음 (내장 전용) | 내부용 |

- Zed 확장 모델(선언형 + WASM)과 같은 방향. VS Code처럼 Node 프로세스에 무제한 권한을 주는 방식은 "가벼움"과 보안 둘 다 해치므로 채택하지 않음
- DB 드라이버는 L4(내장)로 시작하고, 수요가 생기면 L2에서 "JDBC 브리지 플러그인"(로컬 JVM 사이드카) 같은 방식으로 확장

## 2. 확장 매니페스트

```toml
# extension.toml
id = "dev.pitwall.lang-kotlin"
name = "Kotlin"
version = "0.3.0"
engines = { pitwall = ">=1.0" }
authors = ["..."]

[contributes.languages.kotlin]
linguist = "Kotlin"                         # GitHub Linguist 언어 이름 (§4.3) — 확장자 · 파일명은 Linguist 데이터 사용
extensions = ["kt", "kts"]                  # (선택) Linguist에 추가로 매핑할 확장자
grammar = "grammars/kotlin.wasm"          # tree-sitter WASM
queries = "queries/kotlin/"                # highlights.scm, outline.scm, tags.scm, injections.scm
line_comment = "//"

[contributes.language_servers.kotlin-lsp]
languages = ["kotlin"]
mason = "kotlin-lsp"                        # Mason registry 패키지 이름 (§4.4) — 설치 정보는 registry에서
# install = { github_release = "...", asset = "..." }   # registry에 없을 때만 직접 정의
command = "kotlin-lsp"                      # registry의 bin 이름 또는 설치 폴더 기준 경로

[contributes.debug_adapters.java]
languages = ["kotlin", "java"]

[contributes.task_detectors.gradle]
files = ["build.gradle", "build.gradle.kts"]
plugin = "detect_gradle"                    # L2 WASM 함수 (선택)

[wasm]
module = "plugin.wasm"
capabilities = ["fs:read:project", "process:spawn:gradlew", "net:github.com"]
```

## 3. 테마

| 항목 | 설계 |
|---|---|
| 구조 | **디자인 토큰** (`ui.background`, `editor.foreground`, `git.added`, `graph.lane.1` ...) → CSS 변수로 주입 |
| 구문 색상 | tree-sitter 캡처 이름 기준 (`keyword`, `function.method`, `string.special` ...) |
| 형식 | `themes/*.json` — 한 파일에 light/dark 변형 포함 가능 |
| 가져오기 | **VS Code 테마 import** (TextMate scope → tree-sitter capture 매핑 테이블), JetBrains `.icls`는 P2 |
| 기타 | OS 다크모드 자동 전환, 아이콘 팩(파일 아이콘), 폰트/밀도(compact/comfortable), 고대비 |
| 일관성 | 커밋 그래프 · DB 그리드 · 터미널(ANSI 16색)까지 같은 토큰을 사용 — canvas 렌더러도 토큰을 구독 |

## 4. 언어

### 4.1 UI 언어 (i18n)
- 메시지 형식: **Fluent**(`.ftl`) — 복수형/성별/어순 대응이 ICU보다 단순, Rust(`fluent-rs`)와 TS(`@fluent/bundle`) 양쪽에 구현 존재
- 코어(알림, 에러, MCP 메시지)와 UI가 같은 번역 리소스를 공유
- 기본 내장: 한국어, 영어. 그 외 언어는 L1 "언어 팩" 확장으로
- 날짜/숫자 포맷은 `Intl`, 단축키 표기는 OS별(⌘ vs Ctrl)
- 커밋 메시지 · 로그 등 사용자 콘텐츠는 번역 대상 아님

### 4.2 코드 언어 지원
언어 하나 = 아래 조각들의 묶음(L1, 필요 시 L2 보조):

| 조각 | 역할 | 없을 때 |
|---|---|---|
| tree-sitter 문법(WASM) + queries | 하이라이트, 아웃라인, 심볼 인덱스, 폴딩 | 플레인 텍스트 |
| LSP 서버 정의 | 진단, 이동, 참조, 자동완성 | tree-sitter 심볼 인덱스로 폴백 |
| DAP 어댑터 정의 | 디버깅 | 실행만 가능 |
| 태스크 감지 규칙 | 실행 태스크 자동 등록 | 수동 실행 구성 |
| 언어 주입 규칙 | 코드 안 SQL/정규식/HTML 하이라이트 | 없음 |

- 기본 언어도 모두 이 구조의 "내장 확장"으로 제공 → 언어 추가 시 코어 수정 불필요

### 4.3 언어 판별 — GitHub Linguist 데이터 (D28)

파일 → 언어 판별은 직접 표를 만들지 않고 **GitHub Linguist**(MIT)의 데이터를 사용한다.

| Linguist 데이터 | Pitwall 용도 |
|---|---|
| `languages.yml` (확장자 · 파일명 · 인터프리터(shebang) · 별칭 · 색상 · 유형) | 파일 언어 판별, 언어 배지 색, 파일 아이콘 매핑 키 |
| `heuristics.yml` (`.h`, `.m`, `.pl` 등 모호한 확장자 판별 규칙) | 모호한 확장자를 내용 일부로 판별 |
| `vendor.yml` · `documentation.yml` | 벤더 · 문서 파일 판별 → 파일 트리 흐림 표시, 검색에서 제외 옵션 |
| 생성 파일 규칙 (`generated` 판별) | diff에서 생성 파일 접기 (lockfile, 빌드 산출물 등) |

**판별 순서**
1. 사용자/프로젝트 설정의 명시적 매핑 (`[languages.mapping] "*.conf" = "nginx"`)
2. `.gitattributes`의 `linguist-language` · `linguist-vendored` · `linguist-generated` · `linguist-documentation` (GitHub와 같은 결과)
3. 에디터 모드라인 (vim / emacs)
4. 파일명 → 확장자(heuristics 포함) → shebang
5. 확장(L1)이 추가한 확장자 매핑
6. 판별 실패 → 플레인 텍스트

**운영**
- 빌드 시 Linguist YAML을 압축 테이블로 변환해 코어(`pw-lang`)에 내장. 업데이트는 스크립트로 주기적 갱신(Linguist 버전 고정)
- 언어 ID는 Linguist 이름 기준 → 언어 확장은 `linguist = "<이름>"`으로 연결 (tree-sitter 문법 · LSP · DAP)
- heuristics의 정규식은 Rust `regex`로 컴파일 (Ruby 전용 문법은 변환 스크립트에서 처리, 실패 항목은 건너뜀 + 로그)
- `THIRD_PARTY_NOTICES.md`에 Linguist MIT 고지

### 4.4 LSP · DAP · 포매터 설치 — Mason registry (D29)

언어 서버 · 디버거 · 포매터의 "어디서 받아 어떻게 설치하나" 정보는 **Mason registry**(Neovim mason.nvim의 패키지 정의 저장소)를 사용한다. Pitwall은 registry를 **데이터로만** 읽고, 설치는 자체 설치기가 한다 (mason.nvim 코드는 사용하지 않음).

| 항목 | 설계 |
|---|---|
| 데이터 | 패키지별 `package.yaml`: 이름 · 설명 · **라이선스(SPDX)** · 언어 · 분류(LSP/DAP/Formatter/Linter) · `source.id`(purl + 버전) · 플랫폼별 자산 · `bin` · `share` |
| 가져오기 | registry 릴리스 산출물(JSON 번들)을 내려받아 `$CACHE/registry/`에 저장, 하루 1회 갱신 확인. 오프라인이면 마지막 캐시 사용 |
| 고정 | Pitwall 릴리스마다 검증한 registry 버전을 기본으로 고정, 설정에서 "최신 registry 사용" 선택 가능 |
| 설치 위치 | `$DATA/tools/<패키지>/<버전>/`, 실행 파일 링크는 `$DATA/tools/bin/` (PATH에 넣지 않고 Pitwall이 절대 경로로 실행) |
| 설치기 (purl 유형별) | `github`(릴리스 자산 다운로드 · 압축 해제), `generic`(URL), `npm`, `pypi`(전용 venv), `cargo`, `golang`, `openvsx`(VS Code 확장 형태 디버거: codelldb · js-debug 등) — 우선 `github` · `generic` · `npm` · `pypi` · `openvsx` 구현 |
| 툴체인 | `npm` · `pypi` · `cargo` · `golang` 설치에 필요한 node · python 등은 **런타임 제공자(03 §5-1, mise 등)**에서 찾음. 없으면 "node가 필요합니다 — mise로 설치" 안내 |
| 검증 | registry에 체크섬이 있으면 검증, 없으면 HTTPS + 버전 고정. 설치 전 **라이선스 · 출처 표시 후 사용자 확인**(최초 1회) |
| 업데이트 | 설치본 버전 vs registry 버전 비교 → 업데이트 알림, 이전 버전 1개 보관(롤백) |
| 언어 확장 연결 | 언어 확장은 `mason = "<패키지명>"`만 적음. registry에 없는 서버만 `install = {...}`로 직접 정의 |
| 사용자 지정 | 설정에서 서버 경로 직접 지정 가능 (시스템에 이미 설치된 서버 사용 — 예: mise로 설치한 `rust-analyzer`) |
| 원격 | Relay가 원격 머신에서 같은 방식으로 설치 (`$DATA/tools/` on remote) |
| 라이선스 | registry 저장소 라이선스는 착수 시 재확인(Apache-2.0으로 알려짐) → 고지. 개별 도구 라이선스는 각 패키지 정의의 SPDX 표시 |

**관리 화면 (F4-5)**: 설치된 도구 목록(이름 · 버전 · 언어 · 라이선스 · 크기), 검색 → 설치/업데이트/제거, 설치 로그 보기

## 5. 플러그인 API (L2 WASM) 개요

- 인터페이스: WIT(WebAssembly Interface Types)로 정의, 버전별 `pitwall:extension@1.x`
- 제공 호스트 함수(권한별):
  - `project`: 루트/설정 읽기
  - `fs`: 프로젝트 범위 읽기(쓰기는 별도 권한)
  - `process`: 허용된 바이너리만 실행
  - `http`: 허용된 도메인만
  - `ui`: 알림, 커맨드 등록, 퀵픽, 상태바 항목
  - `events`: 파일/깃/실행 이벤트 구독
  - `mcp`: MCP 툴 추가 등록 (확장 툴은 이름 앞에 확장 id가 붙음)
- 권한은 설치 시 사용자에게 표시하고 승인받음, 프로젝트별로 끌 수 있음
- 플러그인 크래시/타임아웃이 코어를 죽이지 않도록 호출마다 fuel/시간 제한

## 6. 배포 · 설치

| 단계 | 방식 |
|---|---|
| v1.0 | 로컬 폴더 설치 (`pitwall ext install ./my-ext`), 개발 모드 핫 리로드 |
| v1.x | Git 저장소 URL 설치, 서명 검증 |
| v2 | 공개 레지스트리 (GitHub 기반 인덱스 저장소 방식) |

- 본체는 **MIT**. 확장은 각자 라이선스를 선언(`license` 필드), 내장 확장은 MIT 또는 허용형 라이선스만

- 프로젝트 추천 확장: `.pitwall/project.toml`의 `recommended_extensions`
- 원격 host에는 "코어 쪽 확장"(L1 언어/LSP 정의, L2)만 동기화, UI 쪽(테마/L3)은 로컬에서만
