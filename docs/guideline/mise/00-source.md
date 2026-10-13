# 런타임 버전의 원천

## 런타임 버전은 `mise.toml` 하나가 정한다 — 클론·워크트리마다 `mise trust && mise install`

클론하거나 워크트리를 만든 직후, 그 디렉터리 안에서 두 명령을 차례로 돌린 뒤에 의존성을 설치한다.

```bash
mise trust     # 경로마다 1회
mise install   # mise.toml 의 버전으로 설치
```

- **Why:** `mise.toml` 은 **경로별로 신뢰**해야 한다. 새 워크트리는 새 경로라 신뢰가 이어지지 않는다. 런타임이 맞기 전에 의존성을 설치하면 이 저장소가 요구하는 런타임이 아니라 **지금 PATH 에 있는 런타임**으로 깔린다.
- **How to apply:**
  - 맞춰졌는지는 이렇게 본다.
    ```bash
    mise doctor          # activated: yes 여야 한다
    mise ls --current    # 각 도구가 mise.toml 을 소스로 잡혀야 한다
    ```
  - 버전을 올릴 때는 `mise.toml` 을 고쳐 커밋한다.
