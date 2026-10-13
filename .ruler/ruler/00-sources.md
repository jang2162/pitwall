# ruler 소스와 생성물

## 지침 소스는 `.ruler/` 이고 에이전트가 읽는 파일은 생성물이다

| 구분 | 파일 |
| --- | --- |
| 소스(수정 대상) | `.ruler/*.md`, `.ruler/skills/` |
| 설정 | `.ruler/ruler.toml` |
| 생성물(직접 수정 금지) | `AGENTS.md`, `CLAUDE.md`, 에이전트별 설정·스킬 경로(`.codex/config.toml`, `.claude/`, `.agents/` 등), 이들의 `.bak` |

생성물에 지침을 써 넣지 않는다. 지침을 고치려면 소스를 고치고 `ruler apply` 를 돌린다.

`.ruler/<스택>/` 과 `docs/guideline/` 은 `guideline` CLI 가 지침 서버에서 받은 **사본**이다. 직접 고치지 않는다 — 고칠 것은 `guideline propose` 로 올린다(에이전트는 `guideline-export` 스킬). 직접 고친 사본은 다음 `guideline update` 에서 건너뛰어지거나 원본과 조용히 갈라진다.

- **Why:** 생성물은 다음 apply 에서 통째로 덮어써진다. 생성물이 소스로 다시 읽히는 설정이면 apply 마다 자기 자신을 덧붙여 중복 헤더로 오염된다.
- **How to apply:** 어떤 파일이 생성물인지 헷갈리면 `.ruler/ruler.toml` 의 에이전트 목록과 `.gitignore` 의 Ruler 블록을 본다. 블록 안에 있으면 생성물이다.

## ruler 는 devDependency 로 고정하고 설치본으로 부른다

`pnpx @intellectronica/ruler apply` 가 아니라 `pnpm exec ruler apply` 로 부른다(또는 그것을 감싼 패키지 스크립트).

- **Why:** `pnpx` 는 `pnpm dlx` 의 별칭이라 `node_modules` 를 무시하고 부를 때마다 레지스트리의 **최신본**을 받는다. 사람과 시점마다 다른 ruler 가 생성물을 만들게 된다. 설치본으로 부르면 lockfile 이 버전의 단일 원천이 되고, 설치가 안 됐으면 조용히 다른 버전을 쓰는 대신 실패한다.
- **How to apply:** `package.json` 의 `devDependencies` 에 `@intellectronica/ruler` 를 넣는다. `prepare` 스크립트에 걸 때도 `pnpm exec ruler apply` 로 쓴다.

## `ruler apply` 는 저절로 돌지 않는다 — 소스를 고친 턴에 직접 돌린다

`prepare` 스크립트에 걸어 둔 apply 는 **install 할 때만** 돈다. 소스를 고쳤다고 돌지 않는다.

- **Why:** 생성물은 gitignore 되어 `git status` 에 뜨지 않는다. 안 돌려도 경고가 없고 리뷰에도 안 걸리므로, 다른 에이전트와 다음 세션이 낡은 지침을 최신인 줄 알고 주입받는다. 원칙과 나머지 근거는 `base` 스택 「생성물을 직접 편집하지 않는다」 절.
- **How to apply:** 저장소 루트에서 apply 를 돌린 뒤 `git status` 를 본다. 생성물은 ignore 되므로 **추적 파일 변경이 없어야 정상**이다(생성물 경로가 새로 생겼으면 `.gitignore` 블록만 바뀐다). 소스 변경만 커밋한다.
  ```bash
  pnpm exec ruler apply
  git status
  ```

## `.gitignore` 의 Ruler 블록은 손으로 고치지 않는다

생성물은 커밋하지 않고, `.gitignore` 의 `# START Ruler Generated Files` … `# END Ruler Generated Files` 블록이 가린다. 이 블록은 apply 가 갱신한다.

- **Why:** apply 가 블록 안을 다시 쓰므로 손으로 넣은 줄은 다음 apply 에서 사라진다.
- **How to apply:** 새 생성물 경로를 가려야 하면 소스(`.ruler/`)나 `ruler.toml` 을 고치고 apply 를 돌려 블록이 스스로 바뀌게 한다. 생성물이 아닌 파일을 가려야 하면 블록 **바깥**에 적는다.
