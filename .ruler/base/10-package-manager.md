# 패키지 매니저

## Node 패키지를 쓰는 저장소는 패키지 매니저로 pnpm 을 쓴다

`package.json` 이 있는 저장소에서 의존성 설치·추가·제거·스크립트 실행은 `pnpm` 으로 한다. `npm`·`yarn` 을 쓰지 않는다.

| 하려는 일 | 명령 |
| --- | --- |
| 설치 | `pnpm install` |
| 추가 | `pnpm add <패키지>` (개발 의존성은 `-D`) |
| 스크립트 실행 | `pnpm <스크립트>` 또는 `pnpm run <스크립트>` |
| 설치본 실행 | `pnpm exec <도구>` |
| 일회성 실행 | `pnpm dlx <도구>` |

- **Why:** 매니저가 섞이면 lockfile 이 둘이 되고(`pnpm-lock.yaml` 과 `package-lock.json`·`yarn.lock`), 어느 쪽이 의존성 버전의 원천인지 갈린다. 다른 매니저로 설치하면 pnpm 이 만든 `node_modules` 구조도 덮어써져 사람과 머신마다 설치 결과가 달라진다.
- **How to apply:**
  - `npm install` 이 습관으로 나오기 쉽다. 명령을 쓰기 전에 `pnpm-lock.yaml` 이 있는지 본다.
  - 다른 매니저의 lockfile(`package-lock.json`, `yarn.lock`)을 만들거나 커밋하지 않는다. 실수로 생겼으면 지운다.
  - 이미 다른 매니저를 쓰는 저장소를 pnpm 으로 옮길지는 사용자에게 묻는다. 말없이 lockfile 을 바꾸지 않는다.
