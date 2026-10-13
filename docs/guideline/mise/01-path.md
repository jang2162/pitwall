# PATH 우선순위

## 실제 버전이 `mise.toml` 과 다르면 PATH 앞자리를 차지한 바이너리를 찾는다

`mise ls --current` 는 `mise.toml` 을 제대로 읽는데 `node -v` 는 다른 버전이 나오는 경우가 있다. mise 가 아니라 **PATH 에서 mise 보다 앞에 있는 다른 바이너리**가 이기는 것이다.

- **Why:** 원인은 대개 셸 설정에서 `mise activate` **뒤에** PATH 앞에 경로를 붙이는 줄(`export PATH="$HOME/.local/bin:$PATH"` 같은 것)이 있는 것이다. mise 는 이미 PATH 어딘가에 있는 경로를 앞으로 다시 끌어오지 않으므로, 그런 줄이 하나라도 뒤에 있으면 진다. 명령은 에러 없이 다른 버전으로 돈다.
- **How to apply:**
  ```bash
  mise ls --current    # mise 가 의도한 버전
  which -a node        # 실제로 PATH 에서 이기는 것
  mise which node      # mise 가 쓰라고 지정한 경로
  ```
  - `mise activate` 를 **PATH 를 건드리는 모든 줄보다 뒤로** 옮긴다.
  - 개인 셸 설정 문제라 저장소 쪽에서는 고칠 수 없다. 사용자에게 어느 줄을 옮겨야 하는지 알린다.
