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

테마 · 외부 도구 · UI 번역 기여(키맵은 [10-ui.md §4.6](10-ui.md)):

```toml
[contributes.themes.dusk]                   # 표 이름 = 테마 id = 테마 파일의 `id` (§3)
file = "themes/dusk.json"

[contributes.external_tools.zed]            # 04 F8-1 · F8-2
label = "Zed"
command = "zed {project_root} {file}:{line}:{column}"   # 변수: {project_root} {file} {line} {column} {branch}
platforms = ["macos", "linux"]              # (선택) macos | linux | windows, 없으면 모든 OS

[contributes.translations.ko]               # 표 이름 = BCP 47 언어 태그
dir = "i18n/ko"                             # 그 언어의 Fluent 리소스(.ftl) 폴더 (§4.1)
```

### 레지스트리 (`crates/pw-ext`)

| 항목 | 규칙 |
|---|---|
| 필수 키 | `id`(`.` 로 나눈 소문자 · 숫자 · `-` 마디 둘 이상), `name`, `version`(semver), `engines.pitwall`(semver 범위). `authors` · `description` · `license` 는 선택 |
| 기여 이름 | `[contributes.<종류>.<이름>]` 의 이름은 소문자 · 숫자 · `-`(UI 번역만 언어 태그) |
| 모르는 것 | 모르는 기여 종류 · 모르는 키는 **경고하고 버린다** — 더 새 Pitwall 용 확장도 이 앱이 아는 기여는 쓴다. 앱이 꼭 알아야 하는 변화는 `engines.pitwall` 로 막는다. 아는 키의 형식이 틀리면 오류 |
| 경로 | 매니페스트의 경로는 확장 폴더 기준 `/` 상대 경로. 절대 경로 · `..` · `\` · `:` 는 오류. 심볼릭 링크로 폴더 밖에 닿는 것도 오류. 가리키는 파일 · 폴더가 없으면 오류 |
| 오류 처리 | 진단에 파일 · 줄 · 열을 붙인다. 오류가 하나라도 있는 확장은 **통째로 등록하지 않는다**(경고만 있으면 등록). 진단은 로그와 `ext.list` 로 나간다 |
| 엔진 버전 | `engines.pitwall` 범위가 앱 버전을 포함하지 않으면 등록하지 않는다. 내장 확장은 앱과 함께 빌드되므로 `"*"` |
| 겹침 | 내장 → 사용자 확장(폴더 이름순) 순서로 읽고, 확장 id · 테마 id · 키맵 id · 외부 도구 id · 언어 id · UI 번역 언어 태그가 먼저 등록된 것과 겹치면 나중 것(그 확장 전체)을 거절한다. 사용자 확장이 내장 기본값을 말없이 가리지 못한다. UI 번역도 같은 언어 태그를 둘 받지 않는다 — i18n(M0-14)이 확장끼리 번역을 병합해야 하면 그때 이 규칙을 다시 정한다 |
| 내용 | 레지스트리는 기여의 메타데이터와 파일 위치만 검사 · 보관한다. 내용은 지금 소비자가 있는 것만 읽는다: 테마는 시작할 때 읽어 겉모양(`id` · `label` · 변형 하나 이상)을 보고(토큰 검사는 화면의 `parseTheme`), 키맵은 `keymap.get` 이 텍스트로 내준다(형식 검사는 키맵 엔진). UI 번역은 시작할 때 언어 폴더의 `.ftl` 을 읽어 Fluent 문법을 검사한다(규칙은 §4.1 「리소스」). 통째로 읽는 이 파일들은 **1 MiB 를 넘으면 오류**다. `[wasm]` 은 읽고 검사만 한다 |
| 내장 확장 | 저장소 `extensions/<폴더>/` 를 **빌드할 때 바이너리에 묶는다**(`crates/pw-ext/build.rs`). Tauri 리소스 폴더처럼 실행 중에 디스크에서 읽지 않는다 — 개발 실행 · 빌드본 · 시험에서 경로가 갈리지 않고, 설치본이 망가져도 기본 테마 · 키맵이 남고, Tauri 가 없는 relay 데몬도 같은 내장 확장을 쓴다. 묶지 않는 것: `.` 으로 시작하는 항목, `~` 로 끝나는 백업, `node_modules`, 심볼릭 링크. 이름이 UTF-8 이 아니면 빌드가 멈춘다 |
| 사용자 확장 | `$DATA/extensions/<폴더>/extension.toml`(03 §5-0). 앱 시작 때 한 번 읽는다. `extension.toml` 이 없는 폴더는 경고하고 건너뛴다. 설치 흐름(§6)은 아직 없다 |
| RPC | `ext.list`(확장 · 진단), `theme.list`, `theme.get { id }`(테마 파일 내용), `keymap.list`, `keymap.get { id }`(키맵 파일 텍스트). UI 번역은 `crates/pw-i18n` 의 `i18n.get` 이 내준다(§4.1) |

## 3. 테마

| 항목 | 설계 |
|---|---|
| 구조 | **디자인 토큰** (`ui.background`, `editor.foreground`, `git.added`, `graph.lane.1` ...) → CSS 변수로 주입 |
| 구문 색상 | tree-sitter 캡처 이름 기준 (`keyword`, `function.method`, `string.special` ...) |
| 형식 | `themes/*.json` — 한 파일에 light/dark 변형 포함 가능. 모양은 `{ id, label, variants: { light: {토큰: 값}, dark: {…} } }`. 토큰 이름은 `.` 로 나눈 영숫자 마디, CSS 변수는 `--pw-` + `.`→`-`(대소문자 유지). 변형은 하나 이상 있어야 하고, 있는 변형은 앱이 아는 토큰을 전부 정의해야 하며 모르는 토큰은 버린다. 앱이 아는 토큰의 정본은 `TOKENS` 목록(`apps/desktop/src/theme/tokens.ts`). **한쪽 변형만 있는 테마는 어느 외관(시스템 · 라이트 · 다크)을 골라도 그 변형을 칠하고**, 네이티브 창(신호등 · 스크롤바 · 메뉴)도 그 외관으로 고정한다. 고른 외관은 저장된 채 남아 두 변형이 있는 테마로 바꾸면 다시 쓰인다(`resolveAppearance`). 기본 테마로 떨어뜨리지 않는 것은 사용자가 고른 테마가 OS 외관이 바뀔 때 사라지지 않게 하려는 것이다 |
| 기본 테마 | 내장 확장 `extensions/theme-default`(§2 「레지스트리」). 화면은 첫 그림을 번들에 묶은 같은 파일로 칠하고, 확장 레지스트리에서 `theme.get` 으로 받은 테마로 바꾼다(기본 키맵도 같은 방식 — 번들 파일로 첫 입력을 받고 `keymap.get` 응답으로 바꾼다, `apps/desktop/src/keymap/default-keymap.ts`) — RPC 왕복을 기다려 칠하면 첫 그림이 토큰 없이 나가 깜빡인다. 받지 못하면 번들 것을 쓰고 경고를 남긴다. 번들보다 먼저 도는 `theme-boot.js` 는 저장된 선택으로 `color-scheme` 만 정한다. 테마를 고르는 설정이 생기면 boot 도 고른 테마에 있는 변형을 알아야 한다(한쪽 변형 테마에서 반대 외관이 한 번 비친다) |
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

구현(`crates/pw-i18n`, 웹뷰 `apps/desktop/src/i18n`):

| 항목 | 규칙 |
|---|---|
| 리소스 | 언어 하나 = `[contributes.translations.<언어 태그>]` 의 `dir` 바로 아래 `.ftl` 파일들(이름순, 하위 폴더 · 다른 확장자는 읽지 않는다). 기본 ko · en 은 내장 확장 `extensions/i18n-default`(메뉴 `menu.ftl`, 공용 `app.ftl`, 웰컴 창 `welcome.ftl`). 레지스트리가 등록할 때 읽어 검사한다 — 파일마다 1 MiB · UTF-8, Fluent 문법 오류는 그 `.ftl` 의 줄 · 열로, 한 언어 안에서 같은 메시지 id 를 두 파일이 정의하면 오류, `.ftl` 이 없는 폴더도 오류다. 오류가 있는 확장은 통째로 빠지고(§2 「오류 처리」) 앱은 나머지 언어로 뜬다. 같은 언어 태그는 먼저 등록된 것이 이긴다(§2 「겹침」) — 언어 팩이 내장 언어의 일부만 고치는 병합은 아직 없다 |
| 언어 설정 | 전역 `settings.toml` 의 `[ui] language`. `"system"`(내장 기본값) 또는 BCP 47 태그. 문자열이 아니거나 태그 모양이 아니면 경고를 남기고 `"system"` 으로 본다 |
| 언어 고르기 | 요청 목록 = 설정한 태그(있으면) → OS 선호 언어들(`sys-locale`). 앞에서부터 처음 맞는 등록 언어를 고른다. 요청 하나는 ① 대소문자 무시 같은 태그 ② 끝 마디를 하나씩 뗀 태그(`zh-Hant-TW` → `zh-Hant` → `zh`) ③ 주 언어가 같은 첫 등록 언어 순서로 맞춘다. 맞는 것이 없으면 `en`. 설정한 언어가 설치돼 있지 않으면 OS 언어로 간다. 고르기는 Rust 한 곳에서만 한다 — 웹뷰는 결과를 받는다 |
| 폴백 | 쓰는 순서 = [고른 언어, `en`]. 고른 언어에 없는 메시지는 `en` 에서, 어디에도 없으면 메시지 id 를 그대로 보인다. `en` 에는 모든 메시지가 있어야 한다. 내장 ko · en 의 메시지 집합이 같은지, 화면 코드 · 네이티브 메뉴가 쓰는 id 가 두 언어에 모두 있는지는 `apps/desktop/src/i18n/resources.test.ts` 가 본다 |
| 양방향 격리 | Fluent 의 FSI · PDI 격리 문자를 넣지 않는다(Rust · TS 둘 다). 오른쪽에서 왼쪽으로 쓰는 언어가 들어오면 다시 정한다 |
| Rust | 네이티브 메뉴 등 Rust 가 그리는 문자열은 `I18n::localizer()` 로 만든다. 메뉴는 언어가 바뀌면 항목의 글자만 바꾼다(다시 만들지 않으므로 동기화한 단축키가 남는다) |
| 웹뷰 | `i18n.get` 이 고른 언어 순서 · 그 리소스(파일 내용 그대로) · 등록된 언어 목록(그 언어의 `language-name` 메시지로 쓴 이름)을 준다. 응답 전의 첫 그림은 번들에 묶은 내장 `.ftl`(같은 파일)로 그린다 — 마지막으로 받은 언어 순서(localStorage), 없으면 브라우저 언어, 없으면 `en`. 받지 못하면 번들 것을 쓰고 경고를 남긴다(테마 · 키맵과 같은 방식, §3 「기본 테마」) |
| 언어 전환 | 설정 파일이 다시 읽혀 `config.changed`(Host 범위)가 나가면 웹뷰는 `i18n.get` 을 다시 부르고 메뉴는 글자를 바꾼다 — 재시작 없이, 손으로 고쳤든 UI 로 바꿨든 같은 길이다. UI: 설정 창 「외관 > 언어」 선택, 액션 찾기의 「언어: 시스템 / <언어 이름>」 액션 |
| 설정 쓰기 | `i18n.setLanguage { language }`(tier write, UI 전용 — MCP 툴로 노출하지 않는다): `"system"` 이나 등록된 언어만 받는다. `settings.toml` 의 `ui.language` 한 키만 고쳐 쓰고(주석 · 서식 · 다른 키 유지, 파일이 없으면 `schema = 1` 로 만든다) 설정을 바로 다시 읽는다. 파일을 읽을 수 없거나 TOML 이 깨졌거나 `ui` 가 표가 아니거나, 설정 계층이 손대지 않는 파일(`schema` 가 앱보다 새 버전이거나 정수가 아님)이면 쓰지 않고 오류다. `schema` 가 없는 옛 파일은 고쳐 쓴다(다시 읽을 때 마이그레이션된다). 심링크는 대상 파일을 고치고, 대상이 없는 링크는 오류다. 다른 프로세스가 같은 파일을 쓰는 경합은 쓰기 직전에 다시 읽어 달라졌으면 그 내용 위에 다시 만든다(3번까지, 넘으면 쓰지 않고 오류) — 그 뒤 rename 까지의 짧은 틈은 막지 못하고 파일 잠금은 두지 않는다 |
| 번역하지 않는 것 | 앱 이름(`Pitwall`, 웰컴 창 제목), 단축키 표기, 로그 메시지, MCP 메시지(M0-17). 아직 옮기지 않은 것: 프로젝트 창(툴 윈도우 이름 · 툴 윈도우 액션), Rust 가 창을 만들 때 붙이는 OS 창 제목, Shell 명령 설치의 관리자 암호 창 안내문(AppleScript 문자열) |

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
