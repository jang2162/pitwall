# nested 모드

`.ruler/ruler.toml` 에 `nested = true` 가 있는 저장소에서만 참이다.

## 하위 디렉터리의 `.ruler/` 가 그 디렉터리 전용 지침의 소스다

루트와 같은 구조를 하위 디렉터리에 두면 ruler 가 그 디렉터리에 생성물을 만들고, 에이전트는 그 디렉터리에서 작업할 때 그것을 주입받는다.

```
<디렉터리>/
├── .ruler/AGENTS.md   ← 소스. 여기만 고친다
├── .ruler/ruler.toml  ← [agents.claude] output_path = "AGENTS.md"
└── AGENTS.md          ← 생성물 (gitignore)
```

루트 `ruler.toml` 의 `[agents.claude]` 는 하위 디렉터리에 적용되지 않는다. 하위 `.ruler/ruler.toml` 에 같은 표가 없으면 그 디렉터리에 `CLAUDE.md` 가 생기고, Claude Code 는 그 디렉터리의 `AGENTS.md` 를 읽지 않는다(→ 「ruler 소스와 생성물」 의 `CLAUDE.md` 절).

그 디렉터리 안에서만 통하는 것(계층 규칙, 컴포넌트 관례, 도메인 구조)만 넣는다. 저장소 공통인 것은 루트에 두고 하위에 다시 쓰지 않는다.

- **Why:** 같은 규칙을 루트와 하위가 함께 가르치면 한쪽만 고쳐질 때 둘이 조용히 어긋난다 — `base` 스택 「가르치는 자리가 여럿이면 전부 맞아야 한다」 절.
- **How to apply:**
  - 길어지면 같은 `.ruler/` 안에서 `00-*.md`, `10-*.md` 처럼 번호를 붙여 나눈다. 이름순으로 이어붙는다.
  - 새 디렉터리에 지침을 붙이는 절차:
    1. `<디렉터리>/.ruler/AGENTS.md` 를 쓰고, `<디렉터리>/.ruler/ruler.toml` 에 `[agents.claude]` 표와 `output_path = "AGENTS.md"` 를 쓴다.
    2. 저장소 루트에서 apply 를 돌린다. 생성물과 `.gitignore` 블록이 함께 만들어진다.
    3. 소스와 `.gitignore` 변경만 커밋한다.

## 지침을 옮기거나 지울 때는 옛 위치의 생성물을 직접 지운다

ruler 는 **소스가 사라진 경로의 생성물을 치워 주지 않는다.**

- **Why:** 소스를 옮기거나 지우고 apply 만 돌리면 옛 위치의 `AGENTS.md`(옛 설정이면 `CLAUDE.md` 도)가 그대로 남는다. 그 디렉터리에서 작업하는 에이전트는 **낡은 지침을 새 지침과 나란히** 계속 주입받는다. gitignore 되어 있어 `git status` 에도 뜨지 않는다.
- **How to apply:**
  - 옮길 때: `git mv` 로 소스를 옮기고 apply 를 돌린 뒤, 옛 위치의 생성물을 지운다.
  - 지울 때: `<디렉터리>/.ruler/` 를 지우고 apply 를 돌린 뒤, 남은 생성물(`AGENTS.md`·`*.bak`, 옛 설정이면 `CLAUDE.md`)을 지운다.
  - 남은 것이 있는지 확인한다.
    ```bash
    git ls-files --others --ignored --exclude-standard --directory | grep -E '(^|/)(AGENTS|CLAUDE)\.md$'
    ```
    나온 경로마다 옆에 `.ruler/` 가 있는지 본다. 없으면 고아 생성물이다. 이 명령은 ignore 된 디렉터리를 통째로 접어 한 줄로 내므로, **디렉터리째 ignore 된 곳 안의 생성물은 보지 못한다.**
