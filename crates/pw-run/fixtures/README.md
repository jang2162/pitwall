# 태스크 감지 픽스처

`crates/pw-run/src/detect/` 의 프로세스를 띄우는 감지기(17-m6-plan M6-05)가 읽는 실제 출력이다. 시험(`src/detect/process_tests.rs`)은
이 파일을 그대로 내보내는 가짜 `mise` · `gradlew` · `mvnw` 로 실행기 전체(신뢰 판정 · 띄우기 · 끝 코드 · stderr 판정 · 해석)를 돌린다.
출력의 격리 폴더 경로는 `/sandbox` 로 바꿨다(시험이 그 자리를 임시 프로젝트 루트로 되돌린다). 그 밖에는 출력 그대로다.

## `mise-tasks/` — `mise tasks ls --json`

| 폴더        | mise                                      | 받은 곳                                                                              |
| ----------- | ----------------------------------------- | ------------------------------------------------------------------------------------ |
| `2025.1.0/` | 2025.1.0 macos-arm64 (ef6936a 2025-01-01) | `https://github.com/jdx/mise/releases/download/v2025.1.0/mise-v2025.1.0-macos-arm64` |
| `2026.4.3/` | 2026.4.3 macos-arm64 (2026-04-03)         | `https://github.com/jdx/mise/releases/download/v2026.4.3/mise-v2026.4.3-macos-arm64` |

`crates/pw-runtime/fixtures/mise/` 와 같은 두 판이다. 프로젝트는 `project/`(`mise.toml` 의 TOML 태스크 넷 — 하나는 `hide` — 와
파일 태스크 `mise-tasks/deploy`). 폴더마다 경우 하나가 `<경우>.stdout` · `.stderr` · `.exit` 다.

| 경우        | 설정                        | 2025.1.0                                        | 2026.4.3                                  |
| ----------- | --------------------------- | ----------------------------------------------- | ----------------------------------------- |
| `trusted`   | `project/`, `mise trust` 뒤 | 끝 코드 0, `name` · `description` · `source` 만 | 끝 코드 0, 칸이 많다(`file` · `hide` · …) |
| `untrusted` | 같은 파일, 신뢰 전          | **끝 코드 0**, stderr 경고, **파일 태스크만**   | 끝 코드 1, stderr 오류, stdout 없음       |
| `broken`    | `mise.toml` 을 `[tasks` 로  | 끝 코드 1                                       | 끝 코드 1(오류 꼴이 다르다)               |

2025 의 `untrusted` 가 감지기가 끝 코드가 아니라 stderr 의 `are not trusted` 로 미신뢰를 가르는 이유다 — 성공으로 끝나면서 반쪽
목록을 낸다. 2025.1.0 은 `--local` 을 모른다(끝 코드 2) — 감지기는 그 옵션을 쓰지 않는다.

만든 방법: 사람의 mise 설치 · 신뢰 목록을 건드리지 않게 `HOME` · `MISE_DATA_DIR` · `MISE_CONFIG_DIR` · `MISE_STATE_DIR` ·
`MISE_CACHE_DIR` 를 임시 폴더로, `NO_COLOR=1`, `PATH=/usr/bin:/bin:/usr/sbin:/sbin` 으로 두고 `project/` 에서
`mise-v<판> tasks ls --json` 을 돌렸다(`mise trust --untrust` → `untrusted`, `mise trust` → `trusted`). 2026.4.3 `broken.stderr` 의
접혀 찍힌 경로 한 곳은 한 줄로 바꿨다.

## `gradle/` — `gradle tasks --all --quiet --console=plain --no-daemon`

Gradle 9.5.0(Temurin 21.0.12)으로 `project/`(루트 + 하위 `app`(application) · `lib`(java-library), 루트의 `hello` 태스크)에서
`GRADLE_USER_HOME` 을 임시 폴더로 두고 돌린 stdout(`9.5.0-tasks-all.stdout`). 태스크 77 개.

## `maven/` — `mvn --batch-mode --quiet help:effective-pom -Doutput=<파일>`

Apache Maven 3.9.9(`https://archive.apache.org/dist/maven/maven-3/3.9.9/binaries/apache-maven-3.9.9-bin.tar.gz`, Temurin 21)으로
`project/`(부모 + 모듈 `core`, `exec-maven-plugin` 의 execution · `maven-surefire-plugin`)에서 `-Dmaven.repo.local` 을 임시 폴더로 두고
쓴 effective POM(`3.9.9-effective-pom.xml` — 여러 모듈이라 `<projects>` 아래 둘).
