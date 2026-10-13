# Orca 가 살아 있을 때

## Orca 가 살아 있으면 워크트리도 Orca 로 만든다

`base` 스택 「워크트리는 저장소 안에 만들고, 하나의 작업이 하나를 갖는다」 의 **생성 명령(`git worktree add`)과 위치(`.worktrees/`)만** 바꾼다. 브랜치·「하나의 작업 = 하나의 워크트리」·정리 규칙은 그 절을 따른다. 만드는 절차와 그 함정은 `orca-worker` 스킬.

일을 시작하기 전에 `orca status --json` 으로 환경을 먼저 확인하고, running runtime 이면 `orca worktree create` 를 쓴다.

- **Why:** 셋 다 `git worktree add` 에는 없다 — ① 반환 JSON 에 워크트리 **id** 가 들어 있어 워커를 붙일 때 그대로 쓴다(따로 찾을 필요가 없다) ② 프로젝트에 설정된 셋업 훅이 돈다(`orca.yaml` 의 `scripts.setup`. 구성은 `06-orca-yaml.md`) ③ 워크트리 생성과 워커 기동을 한 번에 할 수 있다.
- **How to apply:**
  - **환경 확인은 워크트리를 만들기 전에 한다.** 만든 뒤에 확인하면 「이미 만들었으니 그냥 여기서 하자」로 새어 나간다.
  - `git worktree add` 로 만든 것도 Orca 는 인식한다(`orca worktree list --json` 에 id 로 나온다). 그러니 **이미 손으로 만들었더라도 워커는 Orca 로 붙인다.**
  - Orca 환경이 아니면 워크트리는 그대로 만들고, 그 안에서 서브에이전트로 진행하거나 사용자에게 워크트리 경로를 알려 직접 세션을 띄우게 한다.
