# mise 출력 픽스처

`crates/pw-runtime/src/mise.rs` 가 읽는 `mise env --json` 의 실제 출력이다. 시험(`src/mise/tests.rs`)은 이 파일을 그대로 내보내는
가짜 `mise` 로 제공자 전체(실행 · 끝 코드 · stderr 판정 · JSON 해석)를 돌린다. 12-m1-plan.md M1-16 의 「고정한 mise 두 버전의
`env --json` 픽스처 파싱 시험」 이 이것이다.

## 출처

| 폴더        | mise                                      | 받은 곳                                                                              |
| ----------- | ----------------------------------------- | ------------------------------------------------------------------------------------ |
| `2025.1.0/` | 2025.1.0 macos-arm64 (ef6936a 2025-01-01) | `https://github.com/jdx/mise/releases/download/v2025.1.0/mise-v2025.1.0-macos-arm64` |
| `2026.4.3/` | 2026.4.3 macos-arm64 (2026-04-03)         | `https://github.com/jdx/mise/releases/download/v2026.4.3/mise-v2026.4.3-macos-arm64` |

2026.4.3 은 저장소의 `mise.toml` 머리 주석이 말하는 개발용 mise 와 같은 판본이다. 2025.1.0 은 형식 차이(아래 「미신뢰」)를
보이는 옛 판본이다.

## 경우

폴더마다 경우 하나가 `<경우>.stdout` · `<경우>.stderr` · `<경우>.exit`(끝 코드) 세 파일이다.

| 경우        | 설정                              | 2025.1.0                                              | 2026.4.3                                |
| ----------- | --------------------------------- | ----------------------------------------------------- | --------------------------------------- |
| `trusted`   | 아래 `mise.toml`, `mise trust` 뒤 | 끝 코드 0, JSON                                       | 끝 코드 0, JSON(같은 꼴)                |
| `untrusted` | 같은 파일, 신뢰 전                | **끝 코드 0**, stderr 경고, 설정을 뺀 JSON(`PATH` 만) | **끝 코드 1**, stderr 오류, stdout 없음 |
| `broken`    | `[tools` 처럼 깨진 TOML           | 끝 코드 1                                             | 끝 코드 1(오류 꼴이 다르다)             |

`untrusted` 의 차이가 제공자가 끝 코드가 아니라 stderr 의 `are not trusted` 로 미신뢰를 가르는 이유다 — 2025 판은 성공으로
끝나면서 프로젝트 설정을 조용히 뺀다.

## 만든 방법

사람의 mise 설치 · `~/.config/mise` · 신뢰 목록을 건드리지 않게 격리한 폴더에서 돌렸다. `$SANDBOX` 는 임시 폴더다.

```sh
export HOME=$SANDBOX/home MISE_DATA_DIR=$SANDBOX/data MISE_CONFIG_DIR=$SANDBOX/config \
  MISE_STATE_DIR=$SANDBOX/state MISE_CACHE_DIR=$SANDBOX/cache NO_COLOR=1
export PATH=/usr/bin:/bin:/usr/sbin:/sbin
mise-v2026.4.3 install node@22          # 격리한 MISE_DATA_DIR 안으로
cd $SANDBOX/proj                         # 아래 mise.toml
mise-v<판> env --json > untrusted.stdout 2> untrusted.stderr; echo $? > untrusted.exit
mise-v<판> trust
mise-v<판> env --json > trusted.stdout 2> trusted.stderr; echo $? > trusted.exit
```

```toml
[tools]
node = "22"

[env]
GREETING = "hello world"
MULTILINE = "first\nsecond"
UNICODE = "한국어 ✓"
_.path = ["./bin"]
```

출력의 격리 폴더 경로는 `/sandbox` 로 바꿨다(mise 가 줄을 접어 찍은 2026.4.3 `broken.stderr` 의 경로 한 곳은 접힌 세 줄을 한
줄로 바꿨다). 그 밖에는 출력 그대로다.
