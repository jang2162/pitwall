import {
  type Binding,
  type ContextLayers,
  evaluateWhen,
  formatSequence,
  type Platform,
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
        // 웰컴 창에만 등록되는 「목록에서 제거」 와 프로젝트 창 에디터의 줄 삭제(10-ui §4.2 「웰컴 창」).
        ["⌘⌫", ["project.removeFromList []", "editor.deleteLine [editorFocus]"]],
        // 터미널의 ⌘W 는 탭 닫기(IntelliJ 와 같다), 나머지는 탭 또는 창 닫기.
        ["⌘W", ["window.close []", "terminal.closeTab [terminalFocus]"]],
      ],
      win: [
        // 웰컴 창에만 등록되는 「목록에서 제거」 와 프로젝트 창 파일 트리의 삭제(10-ui §4.2 「웰컴 창」 · §5.4).
        ["Delete", ["project.removeFromList []", "fileTree.delete [fileTree]"]],
      ],
      linux: [
        // 웰컴 창에만 등록되는 「목록에서 제거」 와 프로젝트 창 파일 트리의 삭제(10-ui §4.2 「웰컴 창」 · §5.4).
        ["Delete", ["project.removeFromList []", "fileTree.delete [fileTree]"]],
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
