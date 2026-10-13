# tiergear 가 있을 때

## tiergear CLI 가 있으면 워커를 붙이기 직전에 `tiergear launch` 로 모델·effort 를 고른다

워커를 붙이는 명령(`worker-start` 등)을 부르기 직전에 `command -v tiergear` 를 본다. 있으면 그 워크트리에 floor 를 쓰고 모델·effort 를 받아, 붙이는 명령의 `--model`·`--effort` 에 넣는다.

```bash
tiergear launch --worktree <워크트리 경로> [역할 범위] -- "<브리프>"
# stdout: claude --model <모델> --effort <effort>
```

**바뀌는 것은 모델을 고르는 자리 하나뿐이다.** 워크트리를 만드는 법, 만든 직후의 세팅, 붙이는 법, 제출 확인, 수신 대기, 정리는 그 프로젝트가 쓰는 절차(`orca-worker` 스킬, 프로젝트 스크립트, 지침의 손 절차)를 그대로 따른다. 역할이 모델을 정하던 규칙은 역할이 tier 범위를 정하는 규칙이 된다(`01-launch.md`).

- **Why:** `tiergear launch --worktree` 는 판단기가 브리프를 읽고 고른 모델·effort 를 주고, 워커 세션 안의 플러그인이 그 범위 안에서만 tier 를 오르내리게 floor 파일을 써 둔다. floor 없이 붙이면 워커의 첫 턴은 브리프가 아니라 Orca 프리앰블로 판단돼 대개 확신도가 낮게 나오고, 역할로 정한 하한·상한도 세션 안에 남지 않는다.
- **How to apply:**
  - **tiergear 가 없는 머신에서도 돌아야 한다.** CLI 가 없거나 `tiergear launch` 가 실패하면 경고 한 줄을 내고 원래의 역할 기본 모델로 진행한다. 워커 기동을 막지 않는다.
  - 사람이 모델을 명시하면(`--model`) tiergear 를 부르지 않는다. 명시한 예외가 판단기를 이긴다.
  - 이미 도는 에이전트에 붙일 때(`--terminal`)는 모델을 바꿀 수 없으므로 부르지 않는다.
  - 워커를 띄우는 프로젝트 스크립트가 있으면 이 한 줄을 그 스크립트의 붙이기 직전에 넣는다. 범위·출력 읽기·테스트는 `01-launch.md` 를 읽는다.
  - `tiergear orca-spawn` 은 워크트리 생성과 붙이기를 한 번에 해서 그 사이의 세팅을 건너뛴다. 생성 직후 세팅이 없는 프로젝트에서만 쓴다(`02-orca-spawn.md`).
