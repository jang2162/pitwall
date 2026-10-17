# 기여 가이드

사람과 에이전트 모두에게 해당한다. 무엇을 만드는지는 [`docs/design/README.md`](docs/design/README.md) 가 정본이다.

## 준비물

- [mise](https://mise.jdx.dev) — Node·pnpm·Rust·cargo-nextest·cargo-deny·cargo-about·actionlint 버전을 `mise.toml` 로 고정해 설치한다. cargo-about 은 소스에서 빌드하므로 처음 `mise install` 이 1분 남짓 더 걸린다.
- mise 가 셸에 활성화돼 있어야 `pnpm`·`cargo` 가 PATH 에 잡힌다. `mise doctor` 에서 `activated: yes` 인지 본다.
- Linux 는 Tauri 2 가 컴파일에 쓰는 시스템 라이브러리(WebKitGTK 4.1 등)를 따로 설치한다. 목록은 [Tauri 2 Prerequisites](https://v2.tauri.app/start/prerequisites/#linux) 이고, CI 의 `Tauri system dependencies` 단계가 같은 목록을 쓴다.
- `git` 이 PATH 에 있어야 한다. 프로젝트 레지스트리 시험(`crates/pw-core/src/registry/tests.rs`)이 실제 `git init`·`git worktree add` 로 워크트리를 만든다(제품 코드는 git 을 띄우지 않는다).
- 그 밖의 도구를 따로 설치하지 않는다. `npm`·`yarn` 을 쓰지 않는다(패키지 매니저는 pnpm).

## 세팅

```bash
mise trust && mise install && pnpm install
```

`pnpm install` 은 `prepare` 스크립트로 git 훅(lefthook)과 에이전트 지침 생성(`ruler apply`)도 함께 설치한다.

## 명령

모두 저장소 루트에서 실행한다. 태스크 목록은 `mise tasks`.

| 하려는 일                                                                                 | 명령                  |
| ----------------------------------------------------------------------------------------- | --------------------- |
| Rust 워크스페이스 빌드 + TS 패키지 빌드                                                   | `mise run build`      |
| 데스크톱 앱 개발 모드 실행(워크트리마다 다른 개발 서버 포트)                              | `mise run dev`        |
| 전체 테스트(Rust nextest + Rust doctest + TS Vitest)                                      | `mise run test`       |
| 전체 검사(rustfmt·clippy·oxfmt·oxlint·tsc·actionlint·RPC 생성물 최신 여부)                | `mise run lint`       |
| 포맷 적용                                                                                 | `mise run fmt`        |
| RPC 선언에서 TS 타입·JSON Schema·클라이언트 테스트 고정 입력 다시 만들기                  | `mise run gen`        |
| 라이선스 검사(cargo-deny licenses·bans·sources, npm) + `THIRD_PARTY_NOTICES.md` 최신 여부 | `mise run licenses`   |
| `THIRD_PARTY_NOTICES.md` 다시 만들기                                                      | `mise run notices`    |
| Rust 취약점·관리 중단 공지 검사(네트워크 필요)                                            | `mise run advisories` |

하위 단위만 돌릴 때:

```bash
cargo nextest run -p pw-core                       # 크레이트 하나(예: pw-core)
cargo nextest run -p pw-core crate_name            # 이름에 crate_name 이 든 테스트만
cargo test -p pw-rpc --doc                         # 크레이트 하나의 doctest(nextest 는 doctest 를 돌리지 않는다)
pnpm --filter @pitwall/domain test                 # 패키지 하나(예: @pitwall/domain)
```

**테스트가 몇 개 실행됐는지 확인한다.** nextest 는 `Summary` 줄(`N test(s) run: N passed`), doctest 는 크레이트마다 `test result: ok. N passed` 줄, Vitest 는 `Tests  N passed` 줄에 나온다. 0개면 통과가 아니라 실행되지 않은 것이다.

Vitest 는 CSS 를 처리하지 않는다 — `?raw` 로 CSS 를 불러도 오류 없이 빈 문자열이 온다. 시험에서 CSS 내용을 보려면 `node:fs` 로 읽는다(`apps/desktop/src/theme/tokens.test.ts`).

데스크톱 앱(`mise run dev`)을 손으로 확인할 때:

- WKWebView 의 HTML5 드래그 앤 드롭(dockview 탭 이동 등)은 합성 입력(`orca computer drag`·CGEvent)으로 시작되지 않는다. 앱 안에서 확인하려면 debug 빌드의 Web Inspector(⌥⌘I) 콘솔에서 dockview api 로 같은 이동(`panel.api.moveTo`)을 부른다 — api 는 `.dv-dockview` 의 가장 가까운 React 요소에서 fiber 를 올라가 `onApi` prop 을 가진 컴포넌트의 첫 state(없으면 `alternate`)에 있다. 드래그 입력 자체는 Chromium(Playwright 로 개발 서버를 연다) 또는 사람이 확인한다.
- 화면이 잠겨 있으면(`ioreg -n Root -d1 -a | grep -A1 CGSSessionScreenIsLocked` 가 true) `orca computer` 는 「AX reads blocked … toggle Orca Computer Use」 로, `screencapture` 는 「could not create image」 로 실패한다. 권한 문제가 아니므로 설정을 바꾸지 말고 잠금을 먼저 본다. 잠긴 동안에도 System Events 로 메뉴 항목을 읽고 누를 수는 있다 — 메뉴 accelerator 는 `AXMenuItemCmdChar`·`AXMenuItemCmdModifiers`(0=⌘, 1=⌘⇧)로 읽히고(동기화된 단축키를 이것으로 확인한다), 종료는 그 pid 의 메뉴 항목을 번호가 아니라 이름(`"Quit pitwall"`)으로 골라 누른다. 그동안 창 안 동작은 같은 개발 서버를 브라우저로 열어 확인하고(IPC 를 거치는 경로는 `$STATE/logs` 로그로 본다) 창 안 확인은 보고에 미검증으로 남긴다. **잠긴 동안 키 입력(System Events `keystroke`, CGEvent)을 보내지 않는다** — 잠금 화면의 암호 칸으로 들어간다.
- `mise run dev` 중 `createContext` 를 부르는 모듈을 고치면 HMR 이 context 를 새로 만들어, 그 context 를 쓰는 컴포넌트가 「Provider 밖에서 …」 오류로 깨진다. 앱을 다시 띄운다.
- dev 앱은 `PITWALL_*_DIR` 을 주지 않으면 사람의 실제 `$CONFIG`·`$DATA`·`$STATE` 를 쓴다(여러 워크트리의 dev 앱과 정식 앱이 같은 `state.db` 를 함께 쓴다). `state.db` 스키마를 올리는 브랜치나 데이터를 망가뜨릴 수 있는 확인은 `PITWALL_DATA_DIR` 등을 임시 폴더로 주고 띄운다 — 스키마를 올린 dev 앱이 실제 파일을 마이그레이션하면 다른 워크트리와 정식 앱은 그 레지스트리를 읽기만 하게 된다.
- debug 빌드의 단일 인스턴스는 **빌드(워크트리)와 `$DATA` 가 모두 같을 때만** 기존 앱으로 전달한다(`apps/desktop/src-tauri/src/instance.rs`). 띄운 앱에 `pitwall <path>` 를 보내 확인하려면 그 앱과 같은 바이너리 · 같은 `PITWALL_DATA_DIR` 로 실행한다 — 시작 로그의 `single-instance key` 가 같아야 한다. 기존 앱이 받은 요청은 그 앱의 `$STATE/logs` 에 `open request` 로 남는다.
- 「Shell 명령 설치」 를 확인할 때는 `PITWALL_SHELL_COMMAND_TARGET` 에 임시 폴더 안의 절대 경로를 주고 앱을 띄운다. 주지 않으면 `/usr/local/bin/pitwall` 을 바꾸고, 권한이 없으면 관리자 암호 창을 띄운다.
- `mise run dev` 는 개발 서버를 그대로 열어 `tauri.conf.json` 의 CSP 가 적용되지 않는다(인라인 스크립트 · `eval` 이 돈다). CSP 아래 동작은 `pnpm --filter @pitwall/desktop tauri build --debug --no-bundle` 로 만든 `target/debug/pitwall` 을 실행해 확인한다(tauri://localhost). 그 바이너리는 다음 `mise run build`·`dev` 가 덮어쓴다.

## RPC 타입 생성

RPC 의 TS 타입·메서드 목록·JSON Schema 는 `rpc_method!` 선언과 그 입출력 Rust 타입에서만 나온다(`docs/design/02-tech-stack.md` §2.6). TS 타입을 손으로 쓰지 않는다.

- **선언이나 그 타입을 바꾸면 `mise run gen` 을 돌려 생성물을 함께 커밋한다.** 생성물은 `packages/rpc-client/src/generated/`(`rpc.ts`·`schema.json`)와 `packages/rpc-client/test/fixtures/`(시험용 메서드의 `rpc.ts`, 실제 Rust 세션이 주고받은 `golden.json`)다. 손으로 고치지 않는다. 최신이 아니면 `mise run lint`·push 전 훅·CI 가 실패한다(`--check` 로 비교만 하고 쓰지 않는다).
- 생성물은 oxfmt 가 포맷하지 않는다(`.oxfmtrc.json` 의 `ignorePatterns`). 포맷하면 생성기 출력과 커밋된 파일이 달라져 최신 검사가 바이트 비교를 할 수 없다. oxlint·tsc 는 생성물도 검사한다.
- 입출력 타입은 `specta::Type` 과 `schemars::JsonSchema` 를 derive 한다. TS 로 내보낼 수 없는 필드는 생성이 실패하며 필드 이름을 알려 준다 — 64비트 정수는 `#[specta(type = pw_rpc::JsNumber)]`(값이 2^53 미만일 때만), `serde_json::Value` 는 `#[specta(type = pw_rpc::JsonValue)]`. `skip_serializing_if` 는 specta 가 타입을 직렬화·역직렬화용 둘로 가르므로 쓰지 않는다(`pw_rpc::RpcError` 의 직렬화 참고). 그래서 출력의 `Option` 필드는 없을 때 `null` 로 나간다. `#[serde(default)]` 를 함께 달아 키가 없어도 받게 하면 TS 는 `?: T | null` 로 나온다(`pw_core::task::TaskProgress`). enum 변형 안의 `#[serde(flatten)]` 은 specta 가 TS 타입에 반영하지 못해(와이어와 달라진다) 생성기가 거부한다 — 변형 안에서는 펼치지 말고 필드로 중첩한다(`pw_core::methods::DemoTaskUpdate`).
- **메서드를 선언한 크레이트는 `crates/pw-rpc-gen` 의 `Cargo.toml` 의존성과 `src/main.rs` 의 `use <크레이트> as _;` 두 곳에 더한다.** 레지스트리(`pw_rpc::methods`)는 링크된 크레이트의 선언만 본다. `use` 를 빠뜨리면 링커가 그 크레이트를 버려 메서드가 오류 없이 생성물에서 빠지고 `--check` 도 통과한다. 그 빠뜨림은 `apps/desktop/src-tauri` 의 `generated_rpc_covers_the_router` 테스트(생성물의 메서드 == 앱의 Host 가 등록한 메서드)가 잡는다. 앱에 Host 나 Router 를 더하면 그 테스트의 비교 대상에도 더한다.
- `schema.json` 의 입력 스키마가 MCP `inputSchema`(object 필수)로 그대로 쓰이는 것은 struct 입력뿐이다. `()` 는 `{"type": "null"}`, enum 은 `oneOf` 루트다.

## 의존성과 라이선스

MIT 프로젝트라 의존성은 허용형 라이선스만 쓴다(`docs/design/02-tech-stack.md` 「라이선스 검사」 행).

- **의존성(Cargo·npm)을 추가·변경하면 `mise run notices` 로 `THIRD_PARTY_NOTICES.md` 를 다시 만들어 함께 커밋한다.** 이 파일은 생성물이라 손으로 고치지 않는다. 최신이 아니면 push 전 훅과 CI 가 실패한다.
- 허용 라이선스 목록의 정본은 `deny.toml` 의 `[licenses].allow` 다. 목록을 고치면 `about.toml` 의 `accepted` 도 같게 고친다(다르면 `mise run licenses` 가 실패한다). npm 도 같은 목록으로 검사한다(`scripts/licenses.mjs`).
- 목록 밖 라이선스는 전역으로 넣지 않는다. 꼭 필요하면 크레이트 단위 예외로 `deny.toml` 의 `[[licenses.exceptions]]` 와 `about.toml` 의 `[<크레이트>] accepted` 두 곳에 같이 넣는다(지금은 Tauri 전이 의존성의 MPL-2.0, 파일 감시 `notify` 의 CC0-1.0).
- 앱 번들에 들어가는 npm 패키지(프론트엔드 코드가 import 하는 것)는 `dependencies` 에 둔다. 배포 여부와 고지 대상은 `package.json` 의 필드로 판정하므로, `devDependencies` 에 두면 번들에 들어가도 고지와 아래 예외 검사에서 빠진다.
- npm 은 개발 의존성에 한해 패키지 단위 예외를 `licenses/npm-exceptions.toml` 에 둔다. 항목마다 `package`·`allow`·`reason` 을 모두 쓴다. 같은 패키지가 배포 의존성(`pnpm licenses list --prod`)에 나타나면 검사가 실패하고, 쓰이지 않는 예외는 경고가 난다(지금은 빌드 도구 경유 `caniuse-lite` 의 CC-BY-4.0).
- 의존성이 아니라 코드·데이터를 저장소에 복사해 넣었으면 그 고지는 `licenses/vendored.md` 에 손으로 쓰고 `mise run notices` 를 돌린다.
- npm 검사는 지금 OS 에 설치된 패키지만 본다. 다른 OS 전용 바이너리 패키지는 CI 의 해당 OS 잡이 본다.

## CI

`.github/workflows/ci.yml` 이 로컬과 같은 mise 태스크를 부른다. 도구 버전은 `mise.toml` 이 정본이다.

| 잡           | 언제                | 하는 일                                                                      | 실패하면                                         |
| ------------ | ------------------- | ---------------------------------------------------------------------------- | ------------------------------------------------ |
| `check`      | PR, main push       | macOS·Linux·Windows 에서 `mise run lint`, `mise run test`, npm 라이선스 검사 | PR 을 막는다                                     |
| `licenses`   | PR, main push       | Linux 에서 `mise run licenses`                                               | PR 을 막는다                                     |
| `advisories` | PR, main push, 매일 | `mise run advisories`                                                        | PR 에서는 경고만, main push·매일 실행에서는 실패 |

## 커밋 메시지

영어 [conventional commits](https://www.conventionalcommits.org) 로 쓴다.

```text
<type>: <요약>
```

- type: `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `build`, `ci`.
- 요약은 영어, 소문자 시작, 명령형, 마침표 없음.
- 커밋 훅이 staged 파일을 포맷하고 push 전에 전체를 검사한다. `--no-verify` 로 건너뛰지 않는다.
- `git add -A`·`git commit -a` 대신 파일 경로를 지정해 stage 한다.

## PR 흐름

1. `main` 에서 작업 브랜치(또는 워크트리)를 만든다. 최상위 체크아웃에서 브랜치를 갈아타지 않는다.
2. 커밋 전에 `mise run fmt`, `mise run lint`, `mise run test` 가 통과하는지 본다.
3. PR 을 올린다. 제목은 커밋 메시지 규칙을 따른다.
