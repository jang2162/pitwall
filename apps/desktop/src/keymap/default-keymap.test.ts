import {
  type Binding,
  type ContextLayers,
  evaluateWhen,
  formatSequence,
  parseKeySequence,
  type Platform,
  sameSequence,
  whenDepth,
} from "@pitwall/keymap";
import { describe, expect, it } from "vitest";

import { terminalAdmits } from "../terminal/terminal-keys";
import { chooseDefaultKeymap, DEFAULT_KEYMAP_TOML, registryKeymapProblem } from "./default-keymap";

describe("registryKeymapProblem", () => {
  const fromRegistry = chooseDefaultKeymap(
    { content: DEFAULT_KEYMAP_TOML, extension: "dev.pitwall.keymap-default" },
    "mac",
  );

  it("does not report a refetch error while the registry keymap is in use", () => {
    expect(
      registryKeymapProblem({ data: { content: "" }, error: new Error("later") }, fromRegistry),
    ).toBeUndefined();
  });

  it("reports a failed first fetch and an unreadable registry file", () => {
    const bundled = chooseDefaultKeymap(undefined, "mac");
    expect(registryKeymapProblem({ error: new Error("no host") }, bundled)).toBe("Error: no host");
    const broken = chooseDefaultKeymap(
      { content: "schema = 1\n[[bindings]]\naction = 1\n", extension: "x" },
      "mac",
    );
    expect(registryKeymapProblem({ data: {}, error: null }, broken)).toBe(broken.error);
    expect(broken.error).toBeTruthy();
  });
});

// 키맵 겹침 검사: 기본 키맵에서 한 키를 둘 이상의 액션이 쓰는 자리. 같은 `when` 이면 어느 쪽이 도는지가 파일 순서에 달려 버리므로
// 없어야 하고, `when` 이 다르면(범위가 갈린다) 아래 표에 이유와 함께 적어 둔 것만 있어야 한다 — 새 키가 기존 키와 겹치면 이 시험이
// 먼저 알린다.
describe("default keymap overlaps", () => {
  const overlaps = (platform: Platform) => {
    const byKey = new Map<string, string[]>();
    for (const b of chooseDefaultKeymap(undefined, platform).bindings) {
      const key = formatSequence(b.sequence, platform);
      byKey.set(key, [...(byKey.get(key) ?? []), `${b.action} [${b.when ?? ""}]`]);
    }
    return [...byKey].filter(([, actions]) => actions.length > 1);
  };

  it.each(["mac", "win", "linux"] as const)(
    "never binds one key to two actions under the same when on %s",
    (platform) => {
      for (const [key, actions] of overlaps(platform)) {
        const whens = actions.map((a) => a.slice(a.indexOf("[")));
        expect(new Set(whens).size, `${key}: ${actions.join(", ")}`).toBe(whens.length);
      }
    },
  );

  it("shares a key across different whens only where listed", () => {
    expect(
      Object.fromEntries((["mac", "win", "linux"] as const).map((p) => [p, overlaps(p)])),
    ).toEqual({
      mac: [
        // 웰컴 창에만 등록되는 「새 프로젝트」 와 테이블 데이터의 행 추가(M3-17 — 테이블 데이터에 포커스, 입력 칸 밖).
        ["⌘N", ["project.new []", "db.tableData.addRow [dbTableDataFocus && !inputFocus]"]],
        // 웰컴 창에만 등록되는 「목록에서 제거」 와 프로젝트 창 에디터의 줄 삭제(10-ui §4.2 「웰컴 창」).
        // …와 테이블 데이터의 행 삭제(M3-17 — 10-ui §4.2 「DB」 의 그리드 행, 입력 칸 밖).
        [
          "⌘⌫",
          [
            "project.removeFromList []",
            "editor.deleteLine [editorFocus]",
            "db.tableData.deleteRow [dbTableDataFocus && !inputFocus]",
          ],
        ],
        // 웰컴 창의 「새 프로젝트」 시트의 만들기와 프로젝트 창 커밋 툴 윈도우 안의 커밋(M2-16).
        // …와 SQL 콘솔의 실행(M3-15 — 콘솔에 포커스가 있을 때만), 테이블 데이터의 변경 제출(M3-17).
        [
          "⌘↩",
          [
            "project.create []",
            "git.commit.run [activeToolWindow == 'commit']",
            "db.console.run [dbConsoleFocus]",
            "db.tableData.submit [dbTableDataFocus]",
          ],
        ],
        // 툴 윈도우 크기 조절(툴 윈도우에 포커스)과 에디터의 다음 · 이전 변경 위치(M2-17) — 10-ui §4.2 「툴 윈도우 · 창」 · 「Git (VCS)」.
        // Win/Linux 의 크기 조절은 `Ctrl+Shift+화살표` 라 겹치지 않는다.
        [
          "⌃⌥⇧↑",
          [
            "toolWindow.stretchUp [toolWindowFocus && !inputFocus]",
            "git.previousChange [editorFocus]",
          ],
        ],
        [
          "⌃⌥⇧↓",
          [
            "toolWindow.stretchDown [toolWindowFocus && !inputFocus]",
            "git.nextChange [editorFocus]",
          ],
        ],
        // 터미널의 ⌘W 는 탭 닫기(IntelliJ 와 같다), 나머지는 탭 또는 창 닫기.
        ["⌘W", ["window.close []", "terminal.closeTab [terminalFocus]"]],
        // 텍스트 에디터의 줄로 이동과 hex 뷰어의 오프셋으로 이동 — hex 뷰어는 `editorFocus` 를 가린다(src/keymap/context.ts).
        ["⌘L", ["editor.gotoLine [editorFocus]", "hex.gotoOffset [hexFocus]"]],
        // 업데이트(풀, M2-20)와 터미널의 새 탭 — IntelliJ 처럼 터미널에 포커스가 있으면 새 탭이다.
        ["⌘T", ["terminal.newTab [terminalFocus]", "git.pull.open []"]],
        // 변경 롤백(M2) 과 테이블 데이터의 변경 되돌리기(M3-17 — 10-ui §4.2 「DB」, 테이블 데이터에 포커스가 있을 때만).
        ["⌥⌘Z", ["git.rollback []", "db.tableData.revert [dbTableDataFocus]"]],
      ],
      win: [
        // 웰컴 창에만 등록되는 「목록에서 제거」 와 프로젝트 창 파일 트리의 삭제(10-ui §4.2 「웰컴 창」 · §5.4).
        ["Delete", ["project.removeFromList []", "fileTree.delete [fileTree]"]],
        [
          "Ctrl+Enter",
          [
            "project.create []",
            "git.commit.run [activeToolWindow == 'commit']",
            "db.console.run [dbConsoleFocus]",
            "db.tableData.submit [dbTableDataFocus]",
          ],
        ],
        ["Ctrl+G", ["editor.gotoLine [editorFocus]", "hex.gotoOffset [hexFocus]"]],
        // 에디터의 줄 삭제와 테이블 데이터의 행 삭제(M3-17 — 10-ui §4.2 「DB」 의 그리드 행).
        [
          "Ctrl+Y",
          [
            "editor.deleteLine [editorFocus]",
            "db.tableData.deleteRow [dbTableDataFocus && !inputFocus]",
          ],
        ],
        // 터미널의 복사와 결과 그리드의 다른 형식으로 복사(M3-16) — 포커스 범위가 갈린다.
        ["Ctrl+Shift+C", ["terminal.copy [terminalFocus]", "db.grid.copyAs [dbGridFocus]"]],
        // 변경 롤백(M2)과 테이블 데이터의 변경 되돌리기(M3-17).
        ["Ctrl+Alt+Z", ["git.rollback []", "db.tableData.revert [dbTableDataFocus]"]],
      ],
      linux: [
        // 웰컴 창에만 등록되는 「목록에서 제거」 와 프로젝트 창 파일 트리의 삭제(10-ui §4.2 「웰컴 창」 · §5.4).
        ["Delete", ["project.removeFromList []", "fileTree.delete [fileTree]"]],
        [
          "Ctrl+Enter",
          [
            "project.create []",
            "git.commit.run [activeToolWindow == 'commit']",
            "db.console.run [dbConsoleFocus]",
            "db.tableData.submit [dbTableDataFocus]",
          ],
        ],
        ["Ctrl+G", ["editor.gotoLine [editorFocus]", "hex.gotoOffset [hexFocus]"]],
        // 에디터의 줄 삭제와 테이블 데이터의 행 삭제(M3-17 — 10-ui §4.2 「DB」 의 그리드 행).
        [
          "Ctrl+Y",
          [
            "editor.deleteLine [editorFocus]",
            "db.tableData.deleteRow [dbTableDataFocus && !inputFocus]",
          ],
        ],
        ["Ctrl+Shift+C", ["terminal.copy [terminalFocus]", "db.grid.copyAs [dbGridFocus]"]],
        // 변경 롤백(M2)과 테이블 데이터의 변경 되돌리기(M3-17).
        ["Ctrl+Alt+Z", ["git.rollback []", "db.tableData.revert [dbTableDataFocus]"]],
      ],
    });
  });
});

// 키맵 겹침 검사: 기본 키맵에서 같은 키에 다른 액션이 같은 깊이로 함께 살아 있으면(엔진은 병합 순서의 뒤를 고른다 — 앞의 것이
// 말없이 죽는다) 실패한다. 포커스 범위는 파일에 나오는 `when` 키마다 그 키를 포커스 층에 둔 것과 아무 포커스도 없는 것이다.
// 터미널 포커스 범위는 창 규칙(`terminalAdmits`)을 거친 뒤 본다. 창마다 다른 액션을 등록하는 것(웰컴 창의 `project.*` 등)은
// 보지 않는다 — 한 파일 안의 같은 범위만 본다.
describe("default keymap conflicts", () => {
  function conflicts(platform: Platform): string[] {
    const bindings = chooseDefaultKeymap(undefined, platform).bindings;
    const keys = new Set(bindings.flatMap((b) => (b.when ? b.when.match(/[A-Za-z_]\w*/g)! : [])));
    const scopes: [string, ContextLayers][] = [
      ["(none)", [{}, {}, {}]],
      ...[...keys].map((k): [string, ContextLayers] => [k, [{ [k]: true }, {}, {}]]),
    ];
    const out: string[] = [];
    for (const [scope, layers] of scopes) {
      const live = bindings.filter(
        (b) => (!b.whenExpr || evaluateWhen(b.whenExpr, layers)) && terminalAdmits(b, layers),
      );
      const byKey = new Map<string, Binding[]>();
      for (const b of live) {
        const k = JSON.stringify(b.sequence);
        byKey.set(k, [...(byKey.get(k) ?? []), b]);
      }
      for (const [k, list] of byKey) {
        const top = Math.max(...list.map((b) => whenDepth(b.whenExpr, layers)));
        const winners = new Set(
          list.filter((b) => whenDepth(b.whenExpr, layers) === top).map((b) => b.action),
        );
        if (winners.size > 1) out.push(`${platform} ${scope} ${k}: ${[...winners].join(", ")}`);
      }
    }
    return out;
  }

  it.each(["mac", "win", "linux"] as const)(
    "has no two actions on one key in one scope (%s)",
    (p) => {
      expect(conflicts(p)).toEqual([]);
    },
  );
});

// 키맵 가림 검사: 툴 윈도우 같은 컨테이너 컨텍스트로만 범위를 정한 바인딩은 그 안의 입력 칸(파일 트리의 빠른 검색 · 터미널 탭 이름 ·
// 터미널 본문인 xterm 의 입력 칸)에서도 살아 있다. 위의 겹침 검사는 「한 키 두 액션」 만 보므로, 앱 액션이 입력 칸의 텍스트 편집
// 키(단어 이동 · 선택, 줄 끝 선택, 단어 지우기)를 가리는 것은 여기서 본다.
describe("default keymap text editing keys in tool window inputs", () => {
  const TEXT_EDITING: Record<Platform, string[]> = {
    mac: ["alt+", "alt+shift+", "cmd+shift+"].flatMap((m) =>
      ["left", "right", "up", "down"].map((k) => m + k),
    ),
    win: [],
    linux: [],
  };
  TEXT_EDITING.win = [
    ...["ctrl+", "ctrl+shift+", "shift+"].flatMap((m) =>
      ["left", "right", "up", "down", "home", "end"].map((k) => m + k),
    ),
    "ctrl+backspace",
    "ctrl+delete",
  ];
  TEXT_EDITING.linux = TEXT_EDITING.win;

  const scopes: [string, ContextLayers][] = [
    ["input in a tool window", [{ inputFocus: true }, { toolWindowFocus: true }, {}]],
    ["terminal body", [{ inputFocus: true, terminalFocus: true }, { toolWindowFocus: true }, {}]],
  ];

  function live(platform: Platform, layers: ContextLayers): Binding[] {
    return chooseDefaultKeymap(undefined, platform).bindings.filter(
      (b) => (!b.whenExpr || evaluateWhen(b.whenExpr, layers)) && terminalAdmits(b, layers),
    );
  }

  it.each(["mac", "win", "linux"] as const)(
    "leaves the text editing keys of an input inside a tool window to the input (%s)",
    (platform) => {
      const keys = TEXT_EDITING[platform].map(parseKeySequence);
      for (const [scope, layers] of scopes) {
        const taken = live(platform, layers)
          .filter((b) => keys.some((k) => sameSequence(k, b.sequence)))
          .map((b) => `${scope} ${formatSequence(b.sequence, platform)}: ${b.action}`);
        expect(taken).toEqual([]);
      }
    },
  );

  it.each(["mac", "win", "linux"] as const)(
    "does not resize tool windows from an input or the terminal body (%s)",
    (platform) => {
      for (const [scope, layers] of scopes)
        expect(
          live(platform, layers)
            .filter((b) => b.action.startsWith("toolWindow.stretch"))
            .map((b) => `${scope} ${formatSequence(b.sequence, platform)}`),
        ).toEqual([]);
      // 입력 칸이 아닌 툴 윈도우(트리 · 알림)에서는 돈다.
      expect(
        live(platform, [{}, { toolWindowFocus: true }, {}]).filter((b) =>
          b.action.startsWith("toolWindow.stretch"),
        ),
      ).toHaveLength(4);
    },
  );
});

// 「DB」 묶음의 틀(M3-20, 14-m3-plan 「공유 부품의 주인」): DB 바인딩은 파일의 한 자리에 모여 있고, 10-ui §4.2 의 DB 표 차례를 따른다.
// 행마다 그 자리에 더한다 — 아래 차례 표에 없는 `db.*` 액션은 표의 그 자리에 끼면 된다.
describe("default keymap DB group", () => {
  const DB_TABLE_ORDER = [
    "db.console.run",
    "db.console.cancel",
    "db.console.open",
    "db.tableData.addRow",
    "db.tableData.deleteRow",
    "db.tableData.submit",
    "db.tableData.revert",
    "db.grid.copyAs",
    "db.explorer.ddl",
  ];

  it.each(["mac", "win", "linux"] as const)(
    "keeps the db bindings together in the order of the 10-ui DB table (%s)",
    (platform) => {
      const actions = chooseDefaultKeymap(undefined, platform).bindings.map((b) => b.action);
      const at = actions.flatMap((a, i) => (a.startsWith("db.") ? [i] : []));
      expect(at.length).toBeGreaterThan(0);
      expect(at[at.length - 1]! - at[0]! + 1, "db bindings are contiguous").toBe(at.length);
      const known = actions.filter((a) => DB_TABLE_ORDER.includes(a));
      expect(known).toEqual(DB_TABLE_ORDER);
    },
  );
});
