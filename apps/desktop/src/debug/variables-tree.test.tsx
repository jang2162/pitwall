// @vitest-environment happy-dom
// 변수 나무(17-m6-plan M6-18 ② · 완료 기준 ③): 처음에는 범위의 줄만 그리고 `debug.variables` 를 부르지 않는다 — 펼칠 때 그 노드
// 하나만 읽는다. 값은 디버그 대상의 글이라 제어 글자를 보이게 바꾸고 자르며, 자식은 쪽마다 이어 읽는다. en · ko.

import { LocalizationProvider } from "@fluent/react";
import type { DebugVariables } from "@pitwall/rpc-client";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { bundledLocalization, bundledTranslator } from "../i18n/testing";
import { debugStrings } from "./strings";
import { LINE_CHARS } from "./value-text";
import { MAX_DEPTH, PAGE, type TreeNode, VariablesTree } from "./variables-tree";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

let root: Root | undefined;
beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
});
afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
});

const variable = (name: string, value: string, ref = 0) => ({
  name,
  value,
  type: null,
  evaluateName: null,
  variablesReference: ref,
  namedVariables: null,
  indexedVariables: null,
});

async function draw(
  roots: TreeNode[],
  load: (ref: number, start: number, count: number) => Promise<DebugVariables>,
  locale = "ko",
) {
  const element = document.createElement("div");
  document.body.append(element);
  root = createRoot(element);
  const T = debugStrings(bundledTranslator(locale));
  await act(async () =>
    root!.render(
      <LocalizationProvider l10n={bundledLocalization(locale)}>
        <VariablesTree roots={roots} load={load} T={T} label={T.panes.variables} />
      </LocalizationProvider>,
    ),
  );
}

const row = (name: string) => document.querySelector<HTMLElement>(`[data-variable="${name}"]`)!;
const settle = () => act(async () => new Promise((r) => setTimeout(r, 5)));

describe("the variables tree", () => {
  it("reads variables only when a node is expanded", async () => {
    const asked: [number, number, number][] = [];
    const children: Record<number, DebugVariables> = {
      1: { variables: [variable("i", "3"), variable("user", "User", 7)], start: 0, count: PAGE },
      7: { variables: [variable("name", '"kim"')], start: 0, count: PAGE },
    };
    await draw(
      [
        { name: "Locals", variablesReference: 1 },
        { name: "Globals", variablesReference: 2 },
      ],
      async (ref, start, count) => {
        asked.push([ref, start, count]);
        return children[ref] ?? { variables: [], start, count };
      },
    );
    await settle();
    expect(asked).toEqual([]);
    expect(row("Locals").closest("[role=treeitem]")!.getAttribute("aria-expanded")).toBe("false");

    await act(async () => row("Locals").click());
    await settle();
    expect(asked).toEqual([[1, 0, PAGE]]);
    expect(row("i").textContent).toContain("3");
    // 펼칠 수 있는 자식도 펼치기 전에는 읽지 않는다.
    expect(row("user")).not.toBeNull();
    expect(asked).toHaveLength(1);

    await act(async () => row("user").click());
    await settle();
    expect(asked).toEqual([
      [1, 0, PAGE],
      [7, 0, PAGE],
    ]);
    expect(row("name").textContent).toContain('"kim"');
    // 접었다 다시 펼쳐도 다시 읽지 않는다.
    await act(async () => row("user").click());
    await act(async () => row("user").click());
    expect(asked).toHaveLength(2);
  });

  it("shows control characters and cuts long values", async () => {
    const long = "x".repeat(LINE_CHARS + 50);
    await draw([{ name: "s", variablesReference: 1 }], async () => ({
      variables: [
        variable("esc", "\u001b[31mred\u001b[0m"),
        variable("long", long),
        variable("rtl", "a‮b"),
      ],
      start: 0,
      count: PAGE,
    }));
    await act(async () => row("s").click());
    await settle();
    expect(row("esc").textContent).toContain("\\x1b[31mred\\x1b[0m");
    expect(row("rtl").textContent).toContain("a\\u{202e}b");
    expect(row("long").textContent!.length).toBeLessThan(LINE_CHARS + 50);
    expect(row("long").textContent).toContain("…");
  });

  it("reads the next page when asked (en)", async () => {
    const asked: number[] = [];
    await draw(
      [{ name: "list", variablesReference: 3 }],
      async (_ref, start, count) => {
        asked.push(start);
        const n = start === 0 ? count : 2;
        return {
          variables: Array.from({ length: n }, (_, i) =>
            variable(`[${start + i}]`, String(start + i)),
          ),
          start,
          count,
        };
      },
      "en",
    );
    await act(async () => row("list").click());
    await settle();
    const more = document.querySelector<HTMLButtonElement>("[data-variables-more]")!;
    expect(more.textContent).toBe(`${PAGE} more…`);
    await act(async () => more.click());
    await settle();
    expect(asked).toEqual([0, PAGE]);
    expect(row(`[${PAGE + 1}]`)).not.toBeNull();
    expect(document.querySelector("[data-variables-more]")).toBeNull();
  });
});

describe("a self-referencing value", () => {
  it("stops expanding at the depth limit", async () => {
    let asked = 0;
    await draw([{ name: "node", variablesReference: 1 }], async (_ref, start, count) => {
      asked += 1;
      return { variables: [variable(`next${asked}`, "Node", 1)], start, count };
    });
    await act(async () => row("node").click());
    await settle();
    for (let i = 1; i < MAX_DEPTH; i++) {
      const next = row(`next${i}`);
      if (!next) break;
      await act(async () => next.click());
      await settle();
    }
    // 깊이 상한까지만 읽고 그 아래는 「너무 깊다」.
    expect(asked).toBe(MAX_DEPTH - 1);
    expect(document.body.textContent).toContain("(너무 깊어 보이지 않는다)");
  });
});
