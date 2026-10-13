# `tiergear orca-spawn` 으로 워커를 띄울 때

`orca-spawn` 은 Orca 워크트리를 만들고, floor 를 쓰고, 판단기가 고른 모델로 워커를 붙이는 것을 한 번에 한다. **워크트리를 만든 직후 해야 할 세팅(gitignore 된 설정 복사, 의존성 설치, 빌드, 워크트리 전용 DB 등)이 있는 프로젝트에서는 쓰지 않는다** — 그 세팅 없이 워커가 뜬다. 그런 프로젝트는 `tiergear launch` 를 쓴다(`01-launch.md`).

역할별 범위는 `01-launch.md` 의 표를 그대로 붙인다.

## Run 안에서 띄운다

코디네이터 터미널에서 `orca orchestration run-create` 로 Run 을 연 뒤에 `orca-spawn` 을 부른다. 그러면 `orca-spawn` 이 안에서 `worker-start` 를 부른다.

- **Why:** Run 밖에서 부르면 `orca-spawn` 은 터미널을 만들고 에이전트가 준비되기를 기다렸다가 브리프를 타이핑한다. 그러면 세 가지가 깨진다.
  - 워커에게 lifecycle 프리앰블이 붙지 않아 `worker_done` 이 오지 않는다. 수신 대기가 아무것도 받지 못한다.
  - 새 워크트리에서 Claude 가 폴더 신뢰 확인을 띄우면 사람이 Orca 에서 120초 안에 눌러야 한다. `orca-spawn` 은 대신 누르지 않는다.
  - 그 안에 준비되지 않으면 브리프를 **보내지 않고** exit 1 로 끝나며, 워크트리에 폴백 셸이 남는다.
- **How to apply:** JSON 의 `dispatch` 가 `null` 이면 Run 밖에서 띄워진 것이다. 그 워커는 `worker_done` 을 보내지 않으니 터미널을 읽어 진행을 본다.

## `--name` 에 `:` 를 쓰지 않는다

`--name` 은 Task 제목이자 **워크트리·브랜치 이름**이 된다. `orca-worker` 의 `<역할:슬러그>` 대신 `<역할>-<슬러그>` 로 쓴다.

- **Why:** git 브랜치 이름에는 `:` 를 쓸 수 없다. `orca-spawn` 은 이름을 검사하지 않고 Orca 의 워크트리 생성에 그대로 넘긴다.
- **How to apply:** 소문자·숫자·`-` 로 짓는다. 같은 역할을 다시 띄우면 슬러그를 바꾼다.

## 브리프를 `!` 나 `/` 로 시작하지 않는다

- **Why:** 에이전트의 프롬프트는 `!` 로 시작하는 줄을 셸 명령으로, `/` 로 시작하는 줄을 슬래시 명령으로 실행한다. 그래서 `orca-spawn` 은 그런 브리프를, 그리고 줄바꿈·탭이 아닌 제어 문자가 든 브리프를 판단기에 묻기 전에 거절한다.
- **How to apply:** 브리프 첫 줄은 할 일을 말하는 문장으로 시작한다. 경로나 명령으로 시작해야 하면 앞에 한 단어를 붙인다.

## 결과 JSON 의 `tier` 와 `judgedTier` 를 읽는다

| 필드 | 뜻 | 볼 것 |
| --- | --- | --- |
| `judgedTier` | 판단기가 고른 tier. 판단기가 실패했거나, tier 를 내지 않았거나, 확신도가 0.5 미만이면 `null` | `null` 이면 판단 없이 standard 를 범위로 자른 값으로 떴다 |
| `tier` | 범위로 자른 뒤 실제로 적용한 tier | `judgedTier` 와 다르면 범위가 판단을 바꾼 것이다 |
| `dispatch.handle` | 워커 터미널 | 브리프 제출을 이 handle 로 확인한다(`base` 「『보냈다』는 『상대가 받았다』가 아니다」) |

- **Why:** 판단기가 실패해도 `orca-spawn` 은 멈추지 않고 standard 로 띄운다. 이 필드를 보지 않으면 "판단기가 고른 모델로 떴다" 고 잘못 보고하게 된다.
- **How to apply:** `judgedTier` 가 `null` 이면 그 사실과 `tier` 를 사용자에게 한 줄로 알린다.

## 실패하면 `orca-spawn` 을 다시 부르지 않는다 — 남은 워크트리에 붙인다

`orca-spawn` 은 부를 때마다 워크트리를 새로 만든다. 실패하면(0 이 아닌 종료) 같은 명령을 다시 부르지 않는다.

- **Why:** 다시 부르면 워크트리가 하나 더 생긴다. 게다가 `worker-start` 가 실패해도 Orca 가 만든 것(워크트리·터미널·Task)이 남아 있을 수 있다.
- **How to apply:**
  1. 오류 메시지와 `orca worktree list --json` 으로 만들어진 워크트리를 찾는다.
  2. 그 워크트리에 `tiergear launch --worktree` 를 같은 범위로 불러 floor 를 쓰고 모델·effort 를 받는다(`01-launch.md`).
  3. 그 값으로 `orca-worker` 스킬의 붙이기 절차(`worker-start --model <모델> --effort <effort>`)를 밟는다.
  4. 남은 폴백 셸이나 빈 Task 는 `orca-worker` 의 정리 절차로 걷는다.

## 리눅스에서는 `ORCA_CLI_COMMAND` 를 확인한다

`orca-spawn` 은 `ORCA_CLI_COMMAND` 가 있으면 그 실행 파일을, 없으면 PATH 의 `orca` 를 부른다.

- **Why:** 리눅스의 맨 `orca` 는 GNOME 스크린리더다(orca 스택 README). 변수가 없으면 `orca-spawn` 이 Orca 가 아닌 프로그램을 부른다.
- **How to apply:** 리눅스에서 `ORCA_CLI_COMMAND` 가 비어 있으면 `orca-ide` 로 두고 부른다.
