# post-checkout

## post-checkout 은 런타임을 맞춘 뒤에 의존성을 설치한다 — `parallel: true` 를 쓰지 않는다

런타임 관리자(mise 등)의 설치 잡을 첫 잡으로 두고, 의존성 설치 잡을 그 뒤에 **순차로** 둔다.

- **Why:** 병렬로 돌거나 순서가 뒤집히면, 의존성 설치가 그 브랜치가 요구하는 런타임이 아니라 **지금 PATH 에 있는 런타임**으로 돈다.
- **How to apply:**
  ```yaml
  # 순차 실행이다 — 런타임이 맞은 뒤에 설치가 돌아야 한다.
  post-checkout:
    jobs:
      - name: runtime
        run: "if command -v mise >/dev/null; then mise trust && mise install; else echo 'mise 가 없어 런타임 버전을 맞추지 못했다'; fi"
      - name: deps
        run: "pnpm install"
  ```
  - 새 워크트리의 `mise.toml` 은 아직 신뢰되지 않아 `mise install` 만 부르면 바로 실패한다. `mise trust` 를 먼저 부른다.
  - 런타임 관리자가 없는 환경에서는 첫 잡이 **안내만 찍고 넘어가게** 한다.
