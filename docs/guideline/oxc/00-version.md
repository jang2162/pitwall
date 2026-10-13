# 도구 버전

## oxfmt·oxlint 는 lockfile 이 고정한 설치본으로 부른다 — `pnpx` 로 부르지 않는다

직접 돌릴 때는 패키지 스크립트나 `pnpm exec oxfmt`·`pnpm exec oxlint` 로 부른다. `pnpx oxfmt` 는 쓰지 않는다.

- **Why:** `pnpx` 는 `pnpm dlx` 라 레지스트리 최신본을 받아 쓴다. oxfmt 는 0.x 라 **마이너마다 출력이 달라지고**, 옛 버전은 지금 커밋된 파일을 「포맷 안 됨」으로 판정한다. 사람마다 다른 버전을 돌리면 **같은 파일이 커밋마다 왕복한다.** 훅에서 이것이 무한 왕복이 되는 경로는 `lefthook` 스택 「훅은 툴체인을 설치본으로 부른다」 절.
- **How to apply:**
  - 지금 어느 버전이 도는지는 이렇게 잰다.
    ```bash
    pnpm exec oxfmt --version   # lockfile 이 고정한 버전
    pnpx oxfmt --version        # 레지스트리 최신본
    ```
