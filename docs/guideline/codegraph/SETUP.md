# codegraph 세팅

## 확인

- `.mcp.json` 등에 `codegraph` 서버가 있다.
- 프로젝트 루트에 `.codegraph/` 가 있다.
- `codegraph status` 가 인덱스 통계를 보여 준다(없으면 「not initialized」).
- 워크트리에서는 그 워크트리 안에 `.codegraph/` 가 있다. 루트의 인덱스는 워크트리의 인덱스가 아니다(→ 「워크트리」).

## 세팅

1. 프로젝트 루트에서 인덱스를 만든다. 만드는 동안 인덱싱이 같이 돈다.
   ```bash
   npx -y @colbymchenry/codegraph init
   ```
   전역에 `codegraph` 를 설치했다면 `codegraph init` 으로 같다.
2. `codegraph status` 로 파일·심볼 수가 0 이 아닌지 본다.
3. `.codegraph/` 를 커밋할지 정한다. 인덱스는 로컬에서 다시 만들 수 있는 생성물이다. 커밋하지 않으려면 `.gitignore` 에 올린다. 정한 뒤 `git status` 로 확인한다.
4. 에이전트 세션을 새로 연다. MCP 는 세션이 시작될 때 붙으므로 이미 열린 세션에는 인덱스가 생겨도 도구가 안 보일 수 있다.

## 워크트리

**워크트리마다 자기 인덱스를 만든다.** 인덱스는 그것을 만든 작업 트리의 코드를 담는다. 워크트리(`.worktrees/<슬러그>/`)에서 조회하면 codegraph 는 위쪽 루트의 `.codegraph/` 를 찾아 쓰는데, 그러면 「This CodeGraph index belongs to a different git working tree」 경고와 함께 **루트 체크아웃의 코드**로 답한다. 워크트리에서만 바뀐 심볼은 결과에 없고, 새 심볼은 「No results」 가 된다.

1. 워크트리를 만든 직후, 그 워크트리 안에서 인덱스를 만든다.
   ```bash
   cd .worktrees/<슬러그> && npx -y @colbymchenry/codegraph init
   ```
2. `codegraph status` 에 위 경고가 없고 파일·심볼 수가 0 이 아닌지 본다.
3. 에이전트 세션은 그 워크트리에서 연다(「세팅」 4 와 같다).
4. Orca 로 워크트리를 만드는 저장소는 이 `init` 을 `orca.yaml` 의 `scripts.setup` 에 넣는다(`01-orca-setup.md`).
5. `.codegraph/` 는 루트의 `.gitignore` 에 올려 두었다면 워크트리에서도 추적되지 않는다. 확인은 워크트리에서 `git status`.

## 유지

- 워크트리의 인덱스도 따로 어긋난다. 그 워크트리 안에서 `codegraph sync`.
- 코드를 크게 바꾼 뒤 조회 결과가 어긋나면 `codegraph sync` 로 변경분을 반영한다. 처음부터 다시 만들 때는 `codegraph index`.
- 인덱스를 쓰지 않기로 하면 `codegraph uninit` 이 `.codegraph/` 를 지운다. MCP 항목도 함께 뺀다.

## 텔레메트리

기본값으로 익명 사용 통계를 보낸다. 끄려면 셸 환경에 `CODEGRAPH_TELEMETRY=0` 을 export 하거나 `npx -y @colbymchenry/codegraph telemetry off` 를 한 번 돌린다.
