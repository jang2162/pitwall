# mise 세팅

## 확인

- 저장소 루트에 `mise.toml` 이 있다.
- `mise doctor` 가 `activated: yes` 이고, `mise ls --current` 의 각 도구가 `mise.toml` 을 소스로 잡는다.

## 세팅

1. 클론이나 워크트리 디렉터리 안에서 `mise trust` 를 돈다. 경로마다 1회다.
2. `mise install` 로 `mise.toml` 의 버전을 설치한다.
3. 그다음에 의존성을 설치한다. 런타임이 맞기 전에 설치하면 지금 PATH 의 런타임으로 깔린다.
