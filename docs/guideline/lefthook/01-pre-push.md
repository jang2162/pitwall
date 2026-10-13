# pre-commit 이 못 보는 것

## pre-commit 이 staged 파일만 고치면 pre-push 에서 저장소 전체를 검사한다

pre-commit 의 포매터·린터는 `{staged_files}` 에만 돌리고 `stage_fixed: true` 로 고친 것을 다시 stage 한다. 그와 별도로 pre-push 에 **저장소 전체**를 검사하는(고치지 않는) 잡을 둔다.

- **Why:** pre-commit 은 그 커밋이 건드린 파일만 본다. 손대지 않은 파일은 포매터 설정·버전이 바뀐 뒤에도 옛 포맷으로 남아 **조용히 드리프트한다.** 병합·rebase 로 들어온 변경도 pre-commit 을 거치지 않는다. 전체 검사가 없으면 이 드리프트를 잡는 자리가 없다.
- **How to apply:**
  ```yaml
  pre-commit:
    parallel: true
    jobs:
      - name: fmt
        glob: "*.{js,ts,json,md}"
        run: "pnpm exec <포매터> {staged_files}"
        stage_fixed: true

  pre-push:
    jobs:
      - name: fmt-check
        run: "pnpm exec <포매터> --check ."
  ```
  - pre-commit 과 pre-push 가 **같은 버전**의 도구를 부르게 한다 — 「훅은 툴체인을 설치본으로 부른다」 절.
  - 커밋에 해당 파일이 없으면 건너뛰는 게이트(`glob` 으로 거른 잡)도 같은 구멍이 있다. 병합으로 흘러든 변경을 잡아야 하는 게이트는 pre-push 에도 건다.
