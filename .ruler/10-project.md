# Pitwall 프로젝트 규칙

## 설계 문서가 결정의 정본이다

`docs/design/` 가 무엇을 만드는지(제품·스택·구조·기능·계획)의 정본이다. 구현이 설계와 갈라지면 코드를 맞추거나, 설계를 고치자고 사용자에게 먼저 말한다. 조용히 어느 한쪽만 바꾸지 않는다. 작업 단위는 `docs/design/11-m0-plan.md` 의 표가 정한다.

- **Why:** 설계와 코드가 말없이 갈라지면 다음 에이전트는 둘 중 어느 쪽이 맞는지 판정할 근거가 없다.
- **How to apply:** 작업을 시작할 때 `docs/design/README.md` 의 문서 표에서 관련 문서를 연다.

## 저장소 구조

| 경로 | 내용 |
| --- | --- |
| `crates/` | Rust 라이브러리·바이너리 크레이트. `Cargo.toml` 의 `members = ["crates/*"]` 로 워크스페이스에 자동 편입된다 |
| `packages/` | TypeScript 공용 패키지(`@pitwall/*`) |
| `apps/` | 실행 앱(데스크톱·relay 데몬). Rust 크레이트는 `Cargo.toml` 의 `members` 에 경로로 하나씩 적는다(데스크톱은 `apps/desktop/src-tauri`) |
| `extensions/` | 내장 확장 |
| `docs/design/` | 설계 문서(정본) |

`pnpm-workspace.yaml` 이 `packages/*`·`apps/*`·`extensions/*` 를 묶는다. 디렉터리를 만들거나 옮길 때는 `docs/design/02-tech-stack.md` 「레포 구조」 절과 맞춘다.

## 빌드·테스트·검사는 mise 태스크로 한다

도구 버전은 `mise.toml` 이 고정한다. 세팅은 `CONTRIBUTING.md` 「세팅」, 명령과 하위 단위 실행법은 「명령」 절을 따른다. 태스크 목록은 `mise tasks`.

- 빌드 `mise run build`, 테스트 `mise run test`, 검사 `mise run lint`, 포맷 적용 `mise run fmt`.
- 데스크톱 앱은 `mise run dev` 로 띄운다. 개발 서버 포트를 저장소 경로로 정해(`apps/desktop/scripts/dev.mjs`) 워크트리끼리 겹치지 않는다. vite·`tauri dev` 를 직접 띄우지 않는다 — `PITWALL_DEV_PORT` 가 없으면 시작하지 않는다.
- **「통과」를 보고하기 전에 실행된 테스트 개수를 읽는다.** 0개면 통과가 아니다(`base` 스택 「통과를 보기 전에」 절). 어느 줄을 읽는지는 `CONTRIBUTING.md` 「명령」 절. Rust 쪽은 태스크가 `--no-tests=fail` 을 줘서 0개면 실패하지만 TS 쪽은 그렇지 않다.
- 커밋 훅(lefthook)은 `--no-verify` 로 건너뛰지 않는다.

## 데스크톱 앱은 이름이 아니라 pid 로 고르고 끈다

데스크톱 앱의 바이너리 이름은 모든 워크트리에서 `pitwall` 이다. 앱을 고르거나 끌 때 이름(`orca computer --app pitwall`, `tell application "pitwall"`, `pkill pitwall`)을 쓰지 않고 그 워크트리의 pid 를 쓴다.

- **Why:** 여러 워크트리가 동시에 앱을 띄우면 이름은 어느 앱이든 집는다. 다른 워크트리의 앱을 보고 확인했다고 적거나, 남의 앱을 꺼서 그쪽 확인을 조용히 깨뜨린다 — 꺼진 쪽에는 오류 없이 앱이 사라진 것으로만 보인다.
- **How to apply:**
  - pid 는 그 워크트리 경로로 찾는다: `pgrep -f '<워크트리>/target/debug/pitwall'`. 이후 `orca computer --app pid:<pid>`, System Events `first process whose unix id is <pid>` 로 다룬다. 끌 때도 그 pid 프로세스의 메뉴 Quit 을 누른다.
  - `orca computer` 는 뒤에 있는 창을 `--window-id` 로 골라도 앞 창의 트리를 줄 수 있다. 창을 바꿀 때는 먼저 System Events 로 그 창에 `AXRaise` 를 보낸다.
  - CGEvent 처럼 화면 좌표를 받는 도구에 창 기준 좌표(`orca computer` 의 스크린샷 · 트리 기준)를 넘기지 않는다. 창 위치(`orca computer list-windows` 의 x · y)를 더해 넘긴다 — 틀리면 오류 없이 빈자리를 누른다.
  - 끝나면 그 pid 가 없고 개발 서버 포트에 LISTEN 이 없는지(`lsof -nP -iTCP:<포트> -sTCP:LISTEN`) 본다.

## 커밋과 PR

커밋 메시지는 `CONTRIBUTING.md` 「커밋 메시지」, PR 은 「PR 흐름」 절을 따른다. stage 는 경로를 명시한다(`base` 스택 「git add -A」 절).
