# pnpm

`mise.toml` 로 pnpm 을 설치하는 저장소에서만 참이다.

## pnpm 은 `npm:` backend 로 설치한다

`[tools]` 에 `pnpm = "<버전>"` 이 아니라 `"npm:pnpm" = "<버전>"` 으로 적는다.

- **Why:** 기본 backend(aqua)로는 pnpm 11 이 설치되지 않는다. pnpm 11 부터 릴리스 asset 이름이 `pnpm-darwin-arm64.tar.gz` 로 바뀌었는데 aqua 레지스트리는 옛 이름(`pnpm-macos-arm64`)을 찾는다(mise 2026.4.3 에서 실측). `npm:` backend 는 pnpm 을 npm 패키지 그대로 받으므로 레지스트리 정의에 기대지 않는다.
- **How to apply:** aqua 쪽이 고쳐졌는지는 `npm:` 을 뗀 `mise.toml` 로 `mise install` 을 돌려 보면 안다. 고쳐지기 전까지는 `npm:` 을 유지한다.

## pnpm 이 어디서 오는지는 `mise which` 가 아니라 `which` 로 본다

- **Why:** Node 배포판에는 corepack 이 딸려 오고 `node/<버전>/bin/pnpm` 이라는 **corepack shim** 이 함께 깔린다. `mise which pnpm` 이 그 shim 을 가리킬 수 있는데, 그것은 `mise.toml` 이 고정한 pnpm 이 아니다.
- **How to apply:**
  ```bash
  which pnpm   # .../installs/npm-pnpm/... 이어야 한다
  ```
