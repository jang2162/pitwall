// @vitest-environment happy-dom
import { describe, expect, it } from "vitest";

import { TerminalTabs } from "./terminal-tabs";

describe("TerminalTabs", () => {
  it("numbers default names, and activates the right neighbour (else the left) of a closed active tab", () => {
    const tabs = new TerminalTabs();
    const [a, b, c] = [tabs.open(), tabs.open(), tabs.open()];
    expect(tabs.store.getState().tabs.map((t) => t.index)).toEqual([1, 2, 3]);
    tabs.activate(b);
    tabs.close(b);
    expect(tabs.store.getState()).toMatchObject({ activeId: c });
    tabs.close(c);
    expect(tabs.store.getState()).toMatchObject({ activeId: a });
    tabs.close(a);
    expect(tabs.store.getState()).toEqual({ tabs: [], activeId: undefined });
    // 번호는 다시 쓰지 않는다 — 닫은 탭과 새 탭의 이름이 겹치지 않게.
    tabs.open();
    expect(tabs.store.getState().tabs[0]!.index).toBe(4);
  });

  it("keeps the active tab when another one closes, and renames back to the default with an empty name", () => {
    const tabs = new TerminalTabs();
    const a = tabs.open();
    const b = tabs.open();
    tabs.close(a);
    expect(tabs.store.getState().activeId).toBe(b);
    tabs.rename(b, "  build  ");
    expect(tabs.store.getState().tabs[0]!.title).toBe("build");
    tabs.rename(b, " ");
    expect(tabs.store.getState().tabs[0]!.title).toBeUndefined();
    tabs.disposeAll();
  });
});
