# 기여 가이드

사람과 에이전트 모두에게 해당한다. 무엇을 만드는지는 [`docs/design/README.md`](docs/design/README.md) 가 정본이다.

## 준비물

- [mise](https://mise.jdx.dev) — Node·pnpm·Rust·cargo-nextest·cargo-deny·cargo-about·actionlint·shellcheck(actionlint 가 워크플로 스크립트 검사에 부른다) 버전을 `mise.toml` 로 고정해 설치한다. cargo-about 은 소스에서 빌드하므로 처음 `mise install` 이 1분 남짓 더 걸린다.
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
| 데스크톱 앱 release 번들의 콜드 스타트·유휴 메모리 측정(macOS, 아래 「성능 기준선」)      | `mise run perf`       |

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
- 화면이 잠겨 있으면(`ioreg -n Root -d1 -a | grep -A1 CGSSessionScreenIsLocked` 가 true) `orca computer` 는 「AX reads blocked … toggle Orca Computer Use」 로, `screencapture` 는 「could not create image」 로 실패한다. 권한 문제가 아니므로 설정을 바꾸지 말고 잠금을 먼저 본다. 잠긴 동안에도 System Events 로 메뉴 항목을 읽고 누를 수는 있다 — 메뉴 accelerator 는 `AXMenuItemCmdChar`·`AXMenuItemCmdModifiers`(0=⌘, 1=⌘⇧)로 읽히고(동기화된 단축키를 이것으로 확인한다), 종료는 그 pid 의 메뉴 항목을 번호가 아니라 이름으로 골라 누른다 — 이름은 UI 언어를 따른다(en `"Quit Pitwall"`, ko `"Pitwall 종료"`). 임시 `PITWALL_CONFIG_DIR` 로 띄운 앱은 언어 설정이 없어 OS 언어를 따르므로, 이름을 고정하려면 그 폴더의 `settings.toml` 에 `schema = 1` 과 `[ui] language = "en"` 을 넣고 띄운다. 그동안 창 안 동작은 같은 개발 서버를 브라우저로 열어 확인하고(IPC 를 거치는 경로는 `$STATE/logs` 로그로 본다) 창 안 확인은 보고에 미검증으로 남긴다. **잠긴 동안 키 입력(System Events `keystroke`, CGEvent)을 보내지 않는다** — 잠금 화면의 암호 칸으로 들어간다.
- `mise run dev` 중 `createContext` 를 부르는 모듈을 고치면 HMR 이 context 를 새로 만들어, 그 context 를 쓰는 컴포넌트가 「Provider 밖에서 …」 오류로 깨진다. 앱을 다시 띄운다.
- dev 앱은 `PITWALL_*_DIR` 을 주지 않으면 사람의 실제 `$CONFIG`·`$DATA`·`$STATE` 를 쓴다(여러 워크트리의 dev 앱과 정식 앱이 같은 `state.db` 를 함께 쓴다). `state.db` 스키마를 올리는 브랜치나 데이터를 망가뜨릴 수 있는 확인은 `PITWALL_DATA_DIR` 등을 임시 폴더로 주고 띄운다 — 스키마를 올린 dev 앱이 실제 파일을 마이그레이션하면 다른 워크트리와 정식 앱은 그 레지스트리를 읽기만 하게 된다.
- debug 빌드의 단일 인스턴스는 **빌드(워크트리)와 `$DATA` 가 모두 같을 때만** 기존 앱으로 전달한다(`apps/desktop/src-tauri/src/instance.rs`). release 빌드는 `PITWALL_DATA_DIR` 를 주지 않았거나 그 값이 **그 실행 환경의** 기본 자리와 같으면 정식 키 하나를 쓰고, 아니면 그 `$DATA` 의 해시가 붙은 키라 정식 앱과 주고받지 않는다. 기본 자리는 실행한 프로세스의 환경(`XDG_DATA_HOME` 포함)으로 정한다 — `XDG_DATA_HOME` 이 다른 셸에서 Finder 로 띄운 앱의 기본 자리를 `PITWALL_DATA_DIR` 로 적어 실행하면 그 실행은 해시 키가 되어, 같은 `state.db` 에 두 번째 인스턴스가 뜬다. 기본이 아닌 `$DATA` 로 뜬 앱(debug 포함)은 macOS 14+ 에서 그 `$DATA` 의 웹뷰 저장소(`~/Library/WebKit/<번들 id 또는 실행 파일 이름>/WebsiteDataStore/<uuid>`, 시작 로그의 `webview data store`)를 쓴다 — 지워지지 않으니 확인이 끝나면 그 폴더를 지운다. macOS 14 미만에서는 기본 저장소(`~/Library/WebKit/<…>/WebsiteData`)에 그대로 쓴다: localStorage 의 `pitwall.i18n.locales`(UI 언어 목록) · `pitwall.keymap.lastGood`(마지막으로 성공한 키맵) · 프로젝트 id 별 도크 배치 세 가지가 그 앱의 임시 설정으로 덮어써지거나 쌓인다. 띄운 앱에 `pitwall <path>` 를 보내 확인하려면 그 앱과 같은 바이너리 · 같은 `PITWALL_DATA_DIR` 로 실행한다 — 시작 로그의 `single-instance key` 가 같아야 한다. 기존 앱이 받은 요청은 그 앱의 `$STATE/logs` 에 `open request` 로 남는다. Windows · Linux dev 빌드는 dev 키마다 웹뷰 데이터 폴더가 새로 생기고 지워지지 않는다 — 임시 `PITWALL_DATA_DIR` 로 띄울수록 쌓인다. 시험이 실행 파일을 띄울 때(`apps/desktop/src-tauri/tests/cli.rs`)는 `PITWALL_TEST_INSTANCE`(영숫자 1~16자)로 키를 그 실행만의 것으로 가른다 — debug 빌드만 이 변수를 듣고 release 는 무시한다.
- 터미널에서 바이너리(`target/debug/pitwall` 등)를 띄우면 앱이 떨어져 나가고 명령은 곧바로 끝난다(`apps/desktop/src-tauri/src/detach.rs`). 앱의 stderr 는 `/dev/null` 이므로 로그는 `$STATE/logs` 에서 본다. pid 는 띄운 명령이 아니라 `pgrep -f` 로 찾는다(떼어 낸 자식은 같은 경로로 뜬다). `mise run dev` 아래(`PITWALL_DEV_PORT`)는 떼지 않는다.
- `pitwall mcp` 를 확인할 때는 앱과 `pitwall mcp` 에 **같은** `PITWALL_*_DIR` 과 `PITWALL_TEST_INSTANCE` 를 준다 — 소켓 이름이 단일 인스턴스 키와 `$STATE` 로 정해져, 하나라도 다르면 앱에 붙지 않고 헤드리스로 돈다(`docs/design/05-mcp.md` 「로컬 소켓과 헤드리스」 절). 어느 쪽으로 돌았는지는 `$STATE/logs/mcp.<날짜>.log` 의 `connected to the running app` · `serving headless` 줄, 호출은 `$STATE/mcp/audit.db` 로 본다. Claude Code 로 확인할 때는 사람의 설정(`~/.claude.json`, 저장소 `.mcp.json`)을 고치지 않고 `claude -p "<지시>" --mcp-config <임시 json> --strict-mcp-config --allowedTools mcp__pitwall__project_list` 로 그 실행에만 서버를 준다 — 지시를 `--allowedTools` 뒤에 두면 그 옵션의 값으로 먹혀 「Input must be provided」 로 끝난다.
- 「Shell 명령 설치」 를 확인할 때는 `PITWALL_SHELL_COMMAND_TARGET` 에 임시 폴더 안의 절대 경로를 주고 앱을 띄운다. 주지 않으면 `/usr/local/bin/pitwall` 을 바꾸고, 권한이 없으면 관리자 암호 창을 띄운다.
- 「외부 도구로 열기」(⌥F1 · ⌘⌥E, `crates/pw-open-with`)를 확인할 때는 사람의 실제 도구(Zed · VS Code · IntelliJ · Orca · Terminal)를 띄우지 않는다. 임시 `PITWALL_CONFIG_DIR` 의 `settings.toml` 에 가짜 도구(받은 argv · cwd · PATH 를 파일에 적는 스크립트)를 `[open_with.tools.<id>]` 로 등록하고 `[open_with] default` 로 고른다. 결과는 `$STATE/logs/core.*.log` 의 `pw_open_with`(띄운 argv · pid · 실패 이유)와 `pw_runtime`(`PATH from the login shell` · `using the process PATH`) 줄로 본다
- **GUI 앱의 PATH(로그인 셸 PATH 가져오기)는 터미널에서 띄운 앱으로 확인하지 못한다.** `mise run dev` · 바이너리 직접 실행은 물론 **`open` 도 호출한 터미널의 환경을 앱에 넘긴다**(macOS 27 에서 확인 — `launchctl getenv PATH` 가 비어 있는데 `open -n -a …` 로 띄운 앱의 PATH 가 터미널 것이었다). launchd 환경 그대로 띄우려면 `pnpm --filter @pitwall/desktop tauri build --debug --bundles app` 로 번들을 만들고 `launchctl submit -l <라벨> -- /usr/bin/env PITWALL_CONFIG_DIR=… PITWALL_DATA_DIR=… PITWALL_STATE_DIR=… PITWALL_CACHE_DIR=… <번들>/Contents/MacOS/pitwall <경로>` 로 띄운다(앱 PATH 는 `ps eww -p <pid>` 로 본다). `submit` 작업은 앱이 끝나도 launchd 가 다시 띄우므로 **끝낼 때는 메뉴 종료가 아니라 `launchctl remove <라벨>`** 로 끝내고 `launchctl list` 에 라벨이 없는지 본다
- 창이 앞에 없으면 `orca computer` 의 키 입력은 `window_not_focused` 로 거절된다. 사람의 포커스를 빼앗지 않고 누르려면 System Events 로 그 pid 의 창에서 웹 요소를 찾아 `AXPress` 한다 — `entire contents` 는 웹뷰 안으로 내려가지 않으므로 `UI elements` 를 재귀로 따라간다. 버튼은 `aria-label` 이 `description`, cmdk 목록 항목은 보이는 글자가 `title` 이다. 창 목록(`window 1`)이 몇 초씩 비는 일이 있어(-1719) 재시도가 필요하다
- `mise run dev` 는 개발 서버를 그대로 열어 `tauri.conf.json` 의 CSP 가 적용되지 않는다(인라인 스크립트 · `eval` 이 돈다). CSP 아래 동작은 `pnpm --filter @pitwall/desktop tauri build --debug --no-bundle` 로 만든 `target/debug/pitwall` 을 실행해 확인한다(tauri://localhost). 그 바이너리는 다음 `mise run build`·`dev` 가 덮어쓴다.

## 성능 기준선

`mise run perf`(macOS)는 데스크톱 앱 release 번들(`target/release/bundle/macos/Pitwall.app`)을 만들고 `crates/pw-perf` 로 콜드 스타트와 유휴 메모리를 여러 번 잰다. 목표는 [`docs/design/06-roadmap.md`](docs/design/06-roadmap.md) 「성능 목표 (검증 기준)」 절이다. debug 빌드 수치는 기준선이 아니다 — 결과의 `build.profile` 이 잰 빌드를 적는다(바이너리가 놓인 `target/<프로필>/` 로 판정).

무엇을 재는가:

- **시작 경로**: CLI 콜드 스타트 — `<번들>/Contents/MacOS/pitwall <프로젝트>` 로 등록된 프로젝트 하나를 연다. 실행 파일은 앱을 떼어 다시 띄우므로(`apps/desktop/src-tauri/src/detach.rs`) 그 비용까지 든다. 웰컴 창을 거치는 경로(인자 없이 실행해 목록에서 열기)는 재지 않는다.
- **시작점**: 하네스가 실행 파일을 띄우기 직전의 벽시계.
- **끝점**: 프로젝트 창의 첫 화면 — 첫 React 커밋 뒤 두 번째 `requestAnimationFrame` 에서 웹뷰가 `perf_first_frame` 명령을 부르고, 앱이 그것을 받은 때의 벽시계(`apps/desktop/src/perf-mark.tsx`, `apps/desktop/src-tauri/src/perf.rs`). IPC 한 번만큼 늦게 찍힌다. 결과에는 떼어 띄운 앱이 `main` 에 들어온 때도 함께 남는다.
- **측정 모드**: 앱은 `PITWALL_PERF_MARK=<파일>` 이 있을 때만 그 파일에 표시(`main` · `first_frame` · 가려질 때마다 `hidden`)를 남긴다. 없으면 프로젝트 창 주소에 `perf=1` 이 붙지 않아 프론트엔드는 표시 컴포넌트를 그리지 않는다. 측정 모드에서 `PITWALL_PERF_PARENT=<pid>` 를 주면 앱은 그 pid(하네스)가 사라질 때 스스로 정상 종료한다.
- **유휴 메모리**: 첫 프레임 10초 뒤(`--idle-secs`)의 `phys_footprint`(Activity Monitor 의 「메모리」 와 같은 값)를 그 앱 인스턴스의 프로세스 전부에서 합친다. 06 의 300MB 는 300 × 10^6 바이트로 보고, 요약의 MB 도 10^6 바이트다. RSS 도 참고로 남긴다 — 공유 페이지를 프로세스마다 다시 세어 footprint 보다 크게 나온다.
- **프로세스 고르기**: WKWebView 의 WebContent · GPU · Networking 은 앱의 자식이 아니라 launchd 가 띄우는 XPC 프로세스이고, 책임 프로세스(responsible pid)를 요청한 쪽에서 물려받는다. 터미널에서 띄우면 앱도 WebKit 도 터미널 앱을 책임 프로세스로 가져 그 터미널의 다른 프로세스와 구별되지 않는다. 그래서 하네스는 실행 파일을 책임 프로세스를 끊고(`responsibility_spawnattrs_setdisclaim`) 띄운다 — 떼어 띄운 앱이 자기 pid 를 책임 프로세스로 갖고, 그 값을 가진 프로세스가 곧 그 인스턴스의 것이다(다른 Pitwall · Safari · 다른 앱의 WebKit 은 섞이지 않는다). 앱의 책임 프로세스가 앱 pid 도 띄운 pid 도 아니면 그 회는 실패한다. 회마다 고른 프로세스와 각자의 값이 결과에 남는다.
- **반복과 캐시**: 준비 1회 + 측정 5회(`--warmup`, `--runs`). 준비 회와 아래 「화면 잠금 · 가려진 창」 으로 제외한 회는 결과에 남기되 중앙값 · 범위에서 뺀다 — 첫 실행은 디스크 캐시가 찬 정도가 다르다. 회 사이에 캐시를 비우지 않으므로(`sudo purge` 를 쓰지 않는다) **warm 디스크 캐시의 새 프로세스**를 잰 수치다.

결과는 `target/perf/<시각>/`(`--out` 으로 바꾼다)의 `result.json`(빌드 · 커밋 · OS · 하드웨어 · 회별 값 · 유효한 회 수 · 중앙값 · 범위)과 `summary.md` 다. 회마다 `run-<N>/` 에 표시 파일(`marks.jsonl`)과 앱 로그(`logs/`)가 남는다. 회의 임시 `PITWALL_*_DIR` 과 프로젝트는 짧은 자리(`$TMPDIR/pw-perf-<하네스 pid>/run-<N>`)에 두고 회가 끝나면 지운다 — 앱의 MCP 소켓(`$STATE/run/<해시>.sock`)은 103바이트를 넘으면 공용 `$TMPDIR/pitwall-<uid>/` 로 넘어가 끝난 뒤에도 남는데, 워크트리 아래 경로는 그 길이를 넘는다. 회 폴더도 넘으면 하네스가 띄우기 전에 실패한다(더 짧은 `TMPDIR` 를 준다).

종료 코드: 0 — 결과를 썼다(목표를 넘어도 0, 기록만 한다). 3 — 결과는 썼지만 유효한 회가 `--runs` 의 절반보다 적다(요약 맨 위에 경고가 붙는다). 1 — 측정이 실패했다(오류 문장에 그 회가 남긴 프로세스 · 소켓이 붙는다). 128 + 신호 번호 — Ctrl+C · 터미널 닫기 · `kill` 로 멈췄다(진행 중인 회의 앱은 끄고 끝난다).

지키는 것과 멈추는 경우:

- **사람의 데이터**: 회마다 임시 `PITWALL_*_DIR` 에 프로젝트 하나를 등록해 쓰고, 물려받은 `PITWALL_*` 변수는 지운다. 기본이 아닌 `$DATA` 로 뜬 측정 앱은 macOS 14+ 에서 그 회만의 웹뷰 저장소(`~/Library/WebKit/dev.pitwall.desktop/WebsiteDataStore/<uuid>`)를 쓰고, 하네스는 회가 끝나면 그 폴더를 지운다 — 사람의 기본 저장소(`…/WebsiteData`)는 건드리지 않는다. macOS 14 미만에서는 위 「명령」 절의 세 localStorage 키가 사람의 기본 저장소에 쓰인다.
- **단일 인스턴스**: 측정 앱은 기본이 아닌 `$DATA` 로 떠 release 에서도 그 `$DATA` 의 키를 갖는다(위 「명령」 절). 그래서 정식 앱이나 다른 워크트리의 번들이 떠 있어도 측정 앱의 인자를 받아 가지 않고, 측정 중에 사람이 띄운 release Pitwall 의 인자도 측정 앱으로 오지 않는다 — 하네스는 다른 Pitwall 이 떠 있어도 시작한다. 대신 앱 로그의 `single-instance key` 가 `$DATA` 로 갈린 키(`.data-` · `.dev-`)가 아니면 그 회를 실패시킨다(규칙이 없는 옛 번들). 회 중에 띄운 프로세스가 앱을 남기지 않고 끝나면 「같은 키의 다른 Pitwall 이 인자를 받아 갔다」로 실패한다.
- **화면 잠금 · 가려진 창**: 화면이 잠겨 있으면 시작하지 않는다. 창이 가려지거나(다른 앱 · 다른 워크트리의 앱 창) 다른 Space 에 있으면 WebKit 이 프레임을 미루고 GPU · WebContent 메모리도 줄여, 그 회의 두 수치가 모두 기준선이 아니다. 그래서 측정 모드의 프론트엔드는 첫 프레임 전에 문서가 가려졌으면 `first_frame` 표시에 `hidden` 을 싣고, 그 뒤에도 가려질 때마다(`visibilitychange`) `hidden` 표시를 남긴다. 하네스는 유휴 표본 시각까지 가려진 회와, 표본 시각까지 0.5초마다 본 화면 잠금이 한 번이라도 걸린 회를 사유(`excluded`)와 함께 제외한다. 표시가 60초(`--timeout-secs`) 안에 오지 않으면 실패한다. 회마다 프로젝트 창이 한 번 앞에 뜬다(기본 6번) — 측정 중에는 그 창을 가리지 않는다.
- **끄기**: 그 앱 pid 에 AppKit 종료 요청(정상 종료라 단일 인스턴스 소켓을 지운다) → SIGTERM → SIGKILL 순으로 끈다. 사람이 띄운 다른 Pitwall 은 건드리지 않는다. 끈 뒤 그 인스턴스의 프로세스나 단일 인스턴스 소켓(`/tmp/<키>_si.sock`)이 남아 있으면 실패한다.
- **하네스가 멈출 때**: 앱은 떼어 띄워져 터미널의 신호가 닿지 않는다. 하네스는 SIGINT · SIGTERM · SIGHUP 과 패닉에서도 진행 중인 회의 앱을 끄고 임시 폴더를 지운 뒤 끝난다. SIGKILL 처럼 정리할 틈이 없는 끝은 앱이 `PITWALL_PERF_PARENT` 로 받은 하네스 pid 를 0.25초마다 보고 그 pid 가 사라지면 스스로 정상 종료한다(하네스 pid 가 그 사이 다른 프로세스에 다시 쓰이면 잡지 못한다). 이때 임시 폴더(`$TMPDIR/pw-perf-<pid>`)와 그 회의 웹뷰 저장소는 남는다.

다시 재기: `mise run perf`(번들을 다시 만든다). 만든 번들을 다시 재기만 할 때는 `cargo run -q --release -p pw-perf -- --app target/release/bundle/macos/Pitwall.app`. 인자는 `mise run perf -- --runs 3` 처럼 넘긴다. 기준선을 갱신할 때는 커밋하지 않은 변경이 없는 트리에서 재고(`commit.dirty` 가 false), `docs/superpowers/reports/<날짜>-perf-baseline-report.md` 와 같은 이름의 `.json` 으로 남긴다.

보지 않는 것: Linux · Windows, 캐시를 비운 진짜 콜드 기동, 웰컴 창 경로, LSP · 프로젝트 여러 개, 06 의 나머지 목표(git 그래프 · 퍼지 검색 · 그리드). CI 의 `perf` 잡은 같은 태스크를 돌려 job summary 와 artifact(`perf-baseline`)로 남긴다 — 러너는 느리고 들쭉날쭉하므로 기준선이 아니라 추세 참고다.

## RPC 타입 생성

RPC 의 TS 타입·메서드 목록·JSON Schema 는 `rpc_method!` 선언과 그 입출력 Rust 타입에서만 나온다(`docs/design/02-tech-stack.md` §2.6). TS 타입을 손으로 쓰지 않는다.

- **선언이나 그 타입을 바꾸면 `mise run gen` 을 돌려 생성물을 함께 커밋한다.** 생성물은 `packages/rpc-client/src/generated/`(`rpc.ts`·`schema.json`)와 `packages/rpc-client/test/fixtures/`(시험용 메서드의 `rpc.ts`, 실제 Rust 세션이 주고받은 `golden.json`)다. 손으로 고치지 않는다. 최신이 아니면 `mise run lint`·push 전 훅·CI 가 실패한다(`--check` 로 비교만 하고 쓰지 않는다).
- 생성물은 oxfmt 가 포맷하지 않는다(`.oxfmtrc.json` 의 `ignorePatterns`). 포맷하면 생성기 출력과 커밋된 파일이 달라져 최신 검사가 바이트 비교를 할 수 없다. oxlint·tsc 는 생성물도 검사한다.
- 입출력 타입은 `specta::Type` 과 `schemars::JsonSchema` 를 derive 한다. TS 로 내보낼 수 없는 필드는 생성이 실패하며 필드 이름을 알려 준다 — 64비트 정수는 `#[specta(type = pw_rpc::JsNumber)]`(값이 2^53 미만일 때만), `serde_json::Value` 는 `#[specta(type = pw_rpc::JsonValue)]`. `skip_serializing_if` 는 specta 가 타입을 직렬화·역직렬화용 둘로 가르므로 쓰지 않는다(`pw_rpc::RpcError` 의 직렬화 참고). 그래서 출력의 `Option` 필드는 없을 때 `null` 로 나간다. `#[serde(default)]` 를 함께 달아 키가 없어도 받게 하면 TS 는 `?: T | null` 로 나온다(`pw_core::task::TaskProgress`). enum 변형 안의 `#[serde(flatten)]` 은 specta 가 TS 타입에 반영하지 못해(와이어와 달라진다) 생성기가 거부한다 — 변형 안에서는 펼치지 말고 필드로 중첩한다(`pw_core::methods::DemoTaskUpdate`).
- **메서드를 선언한 크레이트는 `crates/pw-rpc-gen` 의 `Cargo.toml` 의존성과 `src/main.rs` 의 `use <크레이트> as _;` 두 곳에 더한다.** 레지스트리(`pw_rpc::methods`)는 링크된 크레이트의 선언만 본다. `use` 를 빠뜨리면 링커가 그 크레이트를 버려 메서드가 오류 없이 생성물에서 빠지고 `--check` 도 통과한다. 그 빠뜨림은 `apps/desktop/src-tauri` 의 `generated_rpc_covers_the_router` 테스트(생성물의 메서드 == 앱의 Host 가 등록한 메서드)가 잡는다. 앱에 Host 나 Router 를 더하면 그 테스트의 비교 대상에도 더한다.
- `schema.json` 의 입력 스키마가 MCP `inputSchema`(object 필수)로 그대로 쓰이는 것은 struct 입력뿐이다. `()` 는 `{"type": "null"}`, enum 은 `oneOf` 루트다.
- **선언에 `mcp: true` 를 단 메서드만 MCP 툴이 된다. 새 메서드는 기본으로 노출되지 않는다** — UI 전용 메서드에는 달지 않는다. 툴을 늘리면 데스크톱 크레이트의 `mcp_tools_are_exactly_these`(`apps/desktop/src-tauri/src/main.rs`) 목록도 함께 고친다 — 모든 메서드 크레이트가 링크되는 자리라 어느 크레이트의 선언이든 거기서 잡힌다(pw-mcp 의 시험은 pw-core 선언만 본다). 입력은 struct 나 `()` 여야 하고 스트림 메서드에는 달 수 없다(이름 · 스키마 규칙은 `docs/design/05-mcp.md` 「툴 레지스트리」 절).

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
| `perf`       | PR, main push       | macOS 에서 `mise run perf`, 결과를 job summary · artifact(`perf-baseline`)로 | 막지 않는다(기록만, 「성능 기준선」)             |

## 릴리스

`.github/workflows/release.yml` 이 태그에서 3 OS 설치 파일과 updater 매니페스트를 만들어 GitHub Release 에 올린다. 결정과 이유는 [`docs/design/02-tech-stack.md`](docs/design/02-tech-stack.md) §3-1 이다.

**태그와 버전.** 버전의 원천은 워크스페이스 `Cargo.toml` 의 `[workspace.package] version` 이다(데스크톱 크레이트가 그것을 쓰고, `tauri.conf.json` 에는 `version` 을 두지 않는다 — `crates/pw-release` 의 시험이 지킨다). `package.json` 의 `version` 은 쓰지 않는다.

- stable 은 `vX.Y.Z`, preview 는 `vX.Y.Z-preview.N`(N 은 0~65535 의 숫자, GitHub Release 를 prerelease 로 낸다). 그 밖의 prerelease(`-rc.1` 등)와 빌드 메타데이터(`+…`)는 거절한다.
- 태그는 그 커밋의 소스 버전 앞에 `v` 를 붙인 것과 같아야 하고, 태그 커밋이 `origin/main` 의 조상이어야 한다(리뷰를 거친 커밋만 공개한다). 둘 중 하나라도 아니면 워크플로의 `plan` 잡이 실패하고 아무것도 만들지 않는다.
- 순서: ① 워크스페이스 버전을 올리는 커밋(`Cargo.lock` 도 함께 바뀐다 — `cargo metadata` 한 번이면 갱신된다)을 PR 로 main 에 병합한다. 그 PR 은 `Cargo.toml` 을 바꾸므로 dry-run 이 저절로 돈다 — 통과를 보고 병합한다. ② main 의 그 커밋에 태그를 달아 push 한다(`git tag v0.1.0-preview.1 <커밋> && git push origin v0.1.0-preview.1`). ③ 워크플로가 3 OS 빌드 → 매니페스트 → draft Release 에 자산을 다 올린 뒤 공개한다.
- **preview 는 Linux 에 AppImage 만 낸다. deb · rpm 은 stable 태그에서만 빌드 · 첨부한다** — 패키지 관리자(dpkg · rpm)가 `X.Y.Z-preview.N` 을 `X.Y.Z` 보다 높게 정렬해, preview 패키지를 설치한 사람이 같은 X.Y.Z 의 stable 로 올라가지 못한다(rpm 은 Version 에 `-` 도 받지 않는다). preview 를 쓰는 Linux 사용자는 자동 업데이트되는 AppImage 를 쓴다. 규칙은 `crates/pw-release` 의 `linux_bundles` 다.
- MSI 는 prerelease 를 숫자로만 받아 preview 의 N 을 넷째 칸에 둔다(`0.1.0-preview.2` → `0.1.0.2`). Windows Installer 는 넷째 칸을 비교하지 않지만 Tauri 의 MSI 는 다른 버전을 늘 덮어 설치한다(`bundle.windows.allowDowngrades` 기본값 true).

**dry-run(공개 없이 시험).** 공개하는 실행(tag push)과 같은 빌드를 하되 Release · 태그를 만들지 않는다 — 쓰기 권한(`contents: write`)이 tag push 에서만 도는 `publish` 잡에만 있다(워크플로 머리 주석).

- **실서명인 것**: macOS 의 Developer ID 서명 · 공증 · 스테이플. 그것이 실제로 통과하는지 보는 것이 dry-run 의 목적이다.
- **일회용 키인 것**: build 잡의 updater 서명. build 잡은 어떤 실행(tag push 포함)에서도 잡 안에서 만든 키로 서명하고 그 키 · 서명을 버린다. tag push 의 공개 실행에서만 `sign-updater` 잡이 build 산출물(`.app.tar.gz` · `.msi` · `.AppImage`)을 운영 키로 다시 서명한다(`tauri signer sign --app-version <버전>` — 빌드 서명과 같은 trusted comment 를 단다). 잡 순서는 공개 실행이 build → sign-updater → manifest → publish, dry-run 이 build → manifest 다.
- **규칙: 운영 updater 키(`secrets.TAURI_SIGNING_*`)를 참조하는 잡은 `sign-updater` 하나뿐이다. 다른 잡에 넣지 않는다 — 조건식 안에도.** Secret 을 참조하는 잡의 runner 에는 그 값이 내려가고, 같은 잡에서 도는 PR 코드(빌드 스크립트 · npm 의존성)가 runner 메모리에서 읽어 낼 수 있다. updater 키는 잃거나 새면 바꿀 수 없다(아래 경고). 그래서 `sign-updater` 는 저장소를 checkout 하지도 빌드하지도 않는다 — lockfile 과 같은 버전의 Tauri CLI 를 `pnpm dlx` 로 받아 서명만 한다. 쓰기 토큰을 가진 `publish` 잡도 저장소를 checkout 하지 않고 plan 출력과 아티팩트만 쓴다. `crates/pw-release/tests/workflow.rs` 가 이 구조(Secret 참조 위치, checkout 없음, 실행 조건, `--app-version`, 도구 버전)를 지킨다.
- **아티팩트에 남는 것(3일)**: 플랫폼마다 `report-<플랫폼>`(산출물 이름과 SHA-256, macOS 는 서명 · 공증 검증 로그)과 `release-manifest`(`latest.json` — 서명 칸은 자리표시 `dry-run`)뿐이다. dmg · msi · AppImage · deb · rpm · `.app.tar.gz` · `.sig` 는 올리지 않는다 — 공개 저장소의 아티팩트는 로그인한 누구나 받을 수 있어, 올리면 병합 안 된 코드의 서명 · 공증 바이너리가 배포된다. 설치 파일을 손으로 받아 보려면 로컬에서 만든다(아래 「로컬에서 updater 확인」).
- PR 이 `.github/workflows/release.yml` · `apps/desktop/src-tauri/tauri.release.conf.json` · `apps/desktop/src-tauri/tauri.conf.json` · `apps/desktop/src-tauri/src/update.rs` · `crates/pw-release/` · `Cargo.toml` · `Cargo.lock` 을 바꾸면 그 PR 에서 돈다(버전을 올린 PR 이 저절로 돈다). 같은 저장소의 브랜치만이다 — 포크 PR 은 Secrets 가 없어 건너뛴다.
- 이 워크플로가 main 에 들어간 뒤로는 아무 브랜치로나 부른다: Actions → Release → Run workflow, 또는 `gh workflow run release.yml --ref <브랜치>`. `workflow_dispatch` 는 기본 브랜치에 파일이 있어야 다른 브랜치로 부를 수 있다. 태그를 골라 불러도 dry-run 이다 — 공개는 plan 이 이벤트(`push`)까지 보고 한 곳에서 판정한다(`crates/pw-release` 의 `plan`).
- 볼 것: macOS 잡의 「Verify signing and notarization」 단계(codesign · `lipo` 두 아키텍처 · `stapler validate` · `spctl` 이 「Notarized Developer ID」, `report-macos` 에도 남는다), `release-manifest` 의 `latest.json`(플랫폼 키 4개).

**publish 를 다시 돌릴 때.** 앞선 실행이 draft 까지 만들고 멈췄으면 그 실행을 다시 돌린다(Actions → 실패한 실행 → Re-run) — publish 잡이 같은 태그의 draft 를 찾아 자산을 덮어 올리고(`gh release upload --clobber`) 공개한다. 이미 공개된 릴리스는 손대지 않고 실패한다 — 공개된 자산을 바꾸면 이미 받은 사람과 바이트가 달라진다. 고쳐야 하면 버전을 올려 새로 낸다. 알려진 틈: 상태를 본 뒤 업로드 전에 사람이 그 draft 를 손으로 공개하면 `--clobber` 가 공개 자산을 바꾼다 — 재실행 중에는 draft 를 손으로 공개하지 않는다.

**매니페스트와 채널.** Release 마다 그 릴리스의 `latest.json` 하나가 자산으로 붙는다. stable 앱은 `releases/latest/download/latest.json`(GitHub 가 prerelease · draft 를 건너뛴다), preview 앱은 GitHub API 의 릴리스 목록에서 `latest.json` 을 가진 가장 높은 버전의 것을 읽는다(`apps/desktop/src-tauri/src/update.rs`). 플랫폼 키는 `darwin-aarch64` · `darwin-x86_64`(유니버설 하나) · `linux-x86_64-appimage` · `windows-x86_64` 다. deb · rpm 설치본은 자동 업데이트하지 않는다 — 맨 `linux-x86_64` 키를 두면 그 설치본이 AppImage 를 받아 설치하려 든다. 그 설치본에서 「업데이트 확인…」 을 고르면 오류 대신 「패키지 관리자로 업데이트한다」 안내가 뜬다.

**태그 ruleset.** 저장소 ruleset 으로 `v*` 태그 생성을 관리자로 제한한다(설정은 사람이 한다 — Settings → Rules → Rulesets, 대상 태그 `v*`, 생성 · 갱신 · 삭제 제한, 우회는 관리자만). 「태그 커밋이 main 의 조상」 검사는 태그 커밋 자신의 `release.yml` 이 집행하므로, 쓰기 권한자가 그 검사를 지운 커밋에 태그를 달면 막지 못한다 — ruleset 이 그 앞을 막는다.

**Secrets.** 저장소 Settings → Secrets and variables → Actions 에 9개가 있어야 한다. 값은 로그에 찍지 않는다(워크플로는 파일로만 쓰고 잡 끝에 지운다).

| Secret                               | 내용                                                       | 다시 만드는 법                                                                                                                                 |
| ------------------------------------ | ---------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| `APPLE_CERTIFICATE`                  | Developer ID Application 인증서 + 개인 키 `.p12` 의 base64 | 키체인 접근에서 인증서를 `.p12` 로 내보내 `base64 -i cert.p12`. 인증서 자체는 developer.apple.com → Certificates 에서 Account Holder 가 만든다 |
| `APPLE_CERTIFICATE_PASSWORD`         | 그 `.p12` 의 암호                                          | 내보낼 때 정한다                                                                                                                               |
| `APPLE_SIGNING_IDENTITY`             | `Developer ID Application: 이름 (TEAMID)`                  | `security find-identity -v -p codesigning` 의 따옴표 안                                                                                        |
| `APPLE_API_ISSUER`                   | App Store Connect API Issuer ID                            | App Store Connect → 사용자 및 액세스 → 통합 → App Store Connect API                                                                            |
| `APPLE_API_KEY`                      | 그 화면의 Key ID                                           | 같은 화면에서 키를 만든다(Developer 이상)                                                                                                      |
| `APPLE_API_KEY_P8`                   | `AuthKey_<KeyID>.p8` 파일의 **내용**                       | 키를 만들 때 한 번만 받는다. 잃으면 키를 폐기하고 새로 만든다(`APPLE_API_KEY` 도 바뀐다)                                                       |
| `TAURI_SIGNING_PRIVATE_KEY`          | updater 서명 개인 키 파일 내용                             | `pnpm --filter @pitwall/desktop exec tauri signer generate -w <파일>` — **새로 만들면 아래 경고**                                              |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | 그 키의 암호                                               | 키를 만들 때 정한다                                                                                                                            |
| `KEYCHAIN_PASSWORD`                  | 러너의 임시 키체인 암호                                    | 아무 무작위 값(`openssl rand -base64 24`)                                                                                                      |

> **⚠ updater 개인 키를 잃으면 이미 설치된 앱은 업데이트를 받지 못한다.** 설치된 앱은 빌드에 들어 있는 공개 키(`tauri.conf.json` 의 `plugins.updater.pubkey`)로만 서명을 검증한다. 개인 키나 그 암호를 잃어 새 키로 서명하면 그 앱들은 모든 새 릴리스를 거절하고, 사람이 새 버전을 직접 받아 설치해야 한다. 키 파일과 암호를 저장소 밖 두 곳 이상(암호 관리자 등)에 보관한다. 키를 바꿀 때도 같다 — 새 공개 키를 넣은 릴리스를 **옛 키로** 서명해 한 번 낸 뒤에 새 키로 넘어간다.

**로컬에서 updater 확인.** 사람의 updater 키와 키체인의 Developer ID 로 로컬 서명하지 않는다(실제 서명 · 공증은 dry-run 몫이다).

- 시험 전용 키 쌍을 임시 폴더에 만들고(`tauri signer generate --ci -p <암호> -w <임시 폴더>/test.key`), release 번들을 `--config src-tauri/tauri.release.conf.json --config <임시 json>` 으로 만든다. 임시 json 에 `version` 과 `plugins.updater` 의 `pubkey`(시험 공개 키) · `dangerousInsecureTransportProtocol: true`(localhost http) 를 넣고, 빌드 환경에 `TAURI_SIGNING_PRIVATE_KEY`(시험 개인 키) · `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 를 준다.
- 매니페스트 주소는 빌드할 때 `PITWALL_UPDATE_STABLE_MANIFEST` · `PITWALL_UPDATE_RELEASES_API`(GitHub API 의 릴리스 목록 꼴 JSON)로 바꾼다. 실행 중 환경 변수로는 바뀌지 않는다.
- 시작할 때의 자동 확인은 release 빌드가 기본 `$DATA` 로 떴을 때만 한다. 임시 `PITWALL_*_DIR` 로 띄운 앱은 메뉴 「업데이트 확인…」(macOS 앱 메뉴, 그 밖은 도움말 메뉴)으로 확인한다. debug 빌드는 메뉴로도 확인하지 않는다.
- 업데이트는 실행 중인 `.app` 을 바꿔 끼우고 다시 띄운다. 시험할 `.app` 은 임시 폴더에 복사해 그것을 띄운다(`target/` 의 번들 · `/Applications` 를 바꾸지 않게). 다시 뜬 앱은 pid 가 바뀐다.

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
