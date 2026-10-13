# base 세팅

## 확인

- `git check-ignore -q .superpowers/handoff/x && echo ignored` 가 `ignored` 를 낸다.
- `git check-ignore -q .worktrees/x && echo ignored` 가 `ignored` 를 낸다.
- `find docs -maxdepth 1 -type f` 가 아무것도 내지 않는다(`docs/` 직속 문서가 없다).
- `docs/` 아래에 한글·대문자·공백이 든 파일 이름이 없다.
- README 가 링크하는 `docs/` 문서가 모두 `guides/` 나 `manual/` 안에 있다.

## 세팅

1. `.gitignore` 에 `.superpowers/` 와 `/.worktrees/` 가 없으면 더한다. 임시 산출물을 두는 저장소는 `/temp/` 도 더한다. Ruler 블록(`# START Ruler Generated Files` … `# END Ruler Generated Files`)이 있으면 그 바깥에 쓴다 — 블록 안은 `ruler apply` 가 다시 쓴다.
   ```gitignore
   # 에이전트 전달 문서와 세션 스크래치
   .superpowers/
   # 에이전트 워크트리
   /.worktrees/
   ```
   `.worktrees/` 는 워크트리를 만드는 자리다(「워크트리를 만들 때」). 가리지 않으면 루트의 `git status` 에 `?? .worktrees/` 가 뜨고, `git add -A` 가 워크트리를 **내장 저장소**로 스테이지한다(「adding embedded git repository」 경고). 워크트리의 내용이 아니라 그 커밋을 가리키는 링크 한 줄이 커밋에 섞인다.
2. 이미 추적 중인 파일이 있으면(`git ls-files .superpowers .worktrees`) 목록을 사용자에게 보이고, 확인을 받아 `git rm -r --cached .superpowers .worktrees` 로 추적에서 뺀다. 없는 경로는 빼고 돌린다.
3. 문서가 놓일 갈래 디렉터리만 만든다(`guides`, `manual`, `adr`, `api`, `ops`, `superpowers/specs`, `superpowers/plans`, `superpowers/reports`). 빈 디렉터리는 git 이 담지 못하므로 첫 문서와 함께 만든다.
4. `docs/` 직속 문서와 이름 규칙에 어긋난 문서를 정리한다.
   1. 문서마다 「언젠가 지우는가?」 로 갈래를 정해 이동 목록(옛 경로 → 새 경로, 바꿀 이름)을 만든다.
   2. 목록을 사용자에게 보이고 확인받는다. 확인 전에는 옮기지 않는다.
   3. `git mv` 로 옮기고, 그 문서를 가리키던 README 링크·지침 색인·코드 주석을 함께 고친다.
5. README 가 지워질 갈래(`ops/`, `superpowers/`, `.superpowers/`)를 링크하면 걷는다.
6. 에이전트가 특정 작업에서 읽어야 하는 `guides/` 문서를 지침 파일의 색인에 「언제 읽는가」 로 올린다.
