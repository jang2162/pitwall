# ruler 세팅

## 확인

- `.ruler/ruler.toml` 의 `[agents.claude]` 표에 `output_path = "AGENTS.md"` 가 있다. nested 모드면 하위 디렉터리마다 `<디렉터리>/.ruler/ruler.toml` 에도 있다.
- 저장소 안에 `CLAUDE.md`·`.claude/CLAUDE.md`·`CLAUDE.local.md` 가 없다. 아래 명령이 아무것도 내지 않아야 한다.

  ```bash
  find . \( -name node_modules -o -name .git \) -prune -o \( -name 'CLAUDE.md' -o -name 'CLAUDE.md.bak' -o -name 'CLAUDE.local.md' \) -print
  ```

## 세팅

1. `.ruler/ruler.toml` 에 claude 의 출력 경로를 넣는다. `[agents.claude]` 표가 이미 있으면 그 안에 `output_path` 줄만 더한다.

   ```toml
   [agents.claude]
   output_path = "AGENTS.md"
   ```

   `default_agents` 에서 `claude` 를 빼지 않는다. 빼면 `.claude/skills/`·`.claude/agents/`·`.mcp.json` 도 생성되지 않는다.
2. nested 모드면 `.ruler/` 가 있는 하위 디렉터리마다 `<디렉터리>/.ruler/ruler.toml` 에 같은 표를 넣는다. 루트의 `[agents.claude]` 는 하위 디렉터리에 적용되지 않아 거기서는 `CLAUDE.md` 가 계속 생성된다.
3. 저장소 루트에서 apply 를 돌린다. `.gitignore` 블록에서 `CLAUDE.md` 줄이 빠진다.
4. 「확인」 의 `find` 명령으로 남은 `CLAUDE.md`·`CLAUDE.md.bak` 를 찾아 지운다. ruler 는 옛 생성물을 치우지 않고, 블록에서 빠진 옛 생성물은 추적되지 않은 파일로 `git status` 에 떠서 커밋될 수 있다. `CLAUDE.local.md` 가 나오면 지우기 전에 사용자에게 묻는다(→ 「Claude Code 도 `AGENTS.md` 를 읽게 하고 저장소에 `CLAUDE.md` 를 두지 않는다」 절).
5. `git status` 에 `.ruler/ruler.toml` 과 `.gitignore` 변경만 있는지 보고 커밋한다.
6. Claude Code 를 새 세션으로 열어 `/memory` 목록에 `AGENTS.md` 경로가 있는지 본다. 없으면 `claude --version` 이 v2.1.281 이상인지 본다. v2.1.276 이하에서 올린 직후의 첫 세션은 읽지 않을 수 있으니 한 번 더 연다.
