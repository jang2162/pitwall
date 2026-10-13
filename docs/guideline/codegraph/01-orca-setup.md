# orca.yaml 셋업에 인덱스 만들기를 넣는다

## codegraph 를 쓰고 Orca 로 워크트리를 만드는 저장소는 `scripts.setup` 에서 인덱스를 만든다 — 실패해도 셋업은 멈추지 않는다

인덱스는 워크트리마다 따로 있어야 한다(`SETUP.md` 「워크트리」). Orca 는 워크트리를 `scripts.setup` 으로 세팅하므로, 인덱스 만들기도 거기 넣는다. 런타임을 맞추고 의존성을 설치한 **다음**에 둔다.

```yaml
scripts:
  setup: |
    set -e
    <런타임을 맞추는 명령>
    <의존성 설치 명령>
    npx -y @colbymchenry/codegraph init || echo "codegraph init 실패: 인덱스 없이 진행"
```

- **Why:** 넣지 않으면 워크트리에는 인덱스가 없다. 루트의 인덱스가 있어도 워크트리에서는 루트 코드로 답하거나 경고를 낸다. 인덱스는 없어도 grep·Read 로 일할 수 있으니 전제 조건이 아니다. 그런데 `set -e` 아래에서 `init` 이 실패하면 셋업이 멈추고, `wait-for-setup` 이면 에이전트가 시작하지 못한다. `|| echo …` 는 실패를 넘기면서 로그에는 남긴다.
- **How to apply:**
  - `|| true` 대신 `|| echo` 를 써서 실패가 셋업 로그에 보이게 한다. 인덱스가 없으면 에이전트는 `00-index.md` 대로 사용자에게 알리고 grep 으로 이어 간다.
  - `.codegraph` 를 `worktree.sharedDirectories` 에 넣지 않는다. 복사나 링크로 가져온 인덱스는 `codegraph sync` 를 하기 전까지 워크트리에서만 바뀐 심볼이 없다.
  - `wait-for-setup` 이면 인덱싱이 끝날 때까지 에이전트 시작이 늦어진다. 저장소가 커서 오래 걸리면 `init` 을 백그라운드로 돌릴지 사람과 정한다.
  - 이미 만든 워크트리에는 소급되지 않는다. 그 워크트리 안에서 `codegraph init` 을 직접 돌린다.
  - 쓴 뒤 워크트리를 하나 만들어 `codegraph status` 에 「different git working tree」 경고가 없고 파일 수가 0 이 아닌지 잰다.
