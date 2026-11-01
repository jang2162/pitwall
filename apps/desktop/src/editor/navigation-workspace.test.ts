// @vitest-environment happy-dom
// 루트 밖 문서의 자리(editor-workspace.tsx 의 `OutsideSources`, 15-m4-plan M4-09 ⓑ · M4-08): 토큰 든 위치는 위치로 가기와
// 뒤로 · 앞으로가 모두 `outside.open` 으로 열고, 외부 소스 탭에서 떠나는 자리는 `outside.locate` 로 기록에 든다. M4-08 은 이
// 둘만 채우면 된다.

import type { FsStat } from "@pitwall/rpc-client";
import type { DockviewApi, IDockviewPanel } from "dockview-react";
import { expect, it, vi } from "vitest";

import { createDocumentStore } from "./documents";
import { EDITOR_PANEL } from "./editor-tabs";
import { createEditorWorkspace, type OutsideSources } from "./editor-workspace";
import type { NavLocation } from "./navigation";

/** 외부 소스 탭만 있는 도킹 영역. 탭 하나 = 토큰 하나. */
function fakeDock() {
  const panels: IDockviewPanel[] = [];
  const api = {
    get panels() {
      return panels;
    },
    activePanel: undefined as IDockviewPanel | undefined,
    getPanel: (id: string) => panels.find((p) => p.id === id),
    onDidActivePanelChange: () => ({ dispose() {} }),
    onDidAddPanel: () => ({ dispose() {} }),
    onDidRemovePanel: () => ({ dispose() {} }),
  };
  return {
    api,
    /** 토큰 `token` 의 외부 소스 탭(없으면 만든다)을 활성으로. */
    show(token: string): IDockviewPanel {
      let panel = panels.find((p) => p.id === token);
      if (!panel) {
        panel = {
          id: token,
          params: { path: `/outside/${token}.rs`, git: { kind: "outside" } },
          api: {
            component: EDITOR_PANEL,
            setActive: () => (api.activePanel = panel),
          },
        } as unknown as IDockviewPanel;
        panels.push(panel);
      }
      api.activePanel = panel;
      return panel;
    },
  };
}

it("opens token places through the outside sources, also when going back and forward", async () => {
  vi.stubGlobal("requestAnimationFrame", (f: () => void) => setTimeout(f, 0));
  const dock = fakeDock();
  const lines = new Map<string, number>([
    ["a", 7],
    ["b", 30],
  ]);
  const opens: { token: string; travel: boolean }[] = [];
  const outside: OutsideSources = {
    open: async (loc, how) => {
      opens.push({ token: loc.token, travel: how.travel });
      if (loc.line !== undefined) lines.set(loc.token, loc.line);
      return dock.show(loc.token);
    },
    locate: (panel): NavLocation => ({
      path: `/outside/${panel.id}.rs`,
      token: panel.id,
      line: lines.get(panel.id) ?? 1,
      column: 1,
    }),
  };
  const stat = (): Promise<FsStat> => Promise.reject(new Error("not a file place"));
  const workspace = createEditorWorkspace({
    documents: createDocumentStore({ stat, read: () => Promise.reject(new Error("none")) }),
    stat,
    onOpenFailed() {},
    onSaveFailed() {},
    onSaveBlocked() {},
    confirmUnsaved: async () => "cancel",
    outside,
  });
  workspace.attach(dock.api as unknown as DockviewApi);
  dock.show("a");

  expect(
    await workspace.navigateTo({ path: "/outside/b.rs", token: "b", line: 30 }, { record: true }),
  ).toBe(true);
  // 외부 소스 탭(a)에서 떠난 자리가 토큰째 든다.
  expect(workspace.navigation.entries()).toEqual([
    { path: "/outside/a.rs", token: "a", line: 7, column: 1 },
  ]);

  expect(await workspace.navigation.back()).toBe(true);
  expect(dock.api.activePanel?.id).toBe("a");
  expect(await workspace.navigation.forward()).toBe(true);
  expect(dock.api.activePanel?.id).toBe("b");
  expect(opens).toEqual([
    { token: "b", travel: false },
    { token: "a", travel: true },
    { token: "b", travel: true },
  ]);
  vi.unstubAllGlobals();
});

it("does not open a token place without outside sources", async () => {
  const stat = (): Promise<FsStat> => Promise.reject(new Error("none"));
  const workspace = createEditorWorkspace({
    documents: createDocumentStore({ stat, read: () => Promise.reject(new Error("none")) }),
    stat,
    onOpenFailed() {},
    onSaveFailed() {},
    onSaveBlocked() {},
    confirmUnsaved: async () => "cancel",
  });
  expect(await workspace.navigateTo({ path: "/outside/x.rs", token: "x", line: 1 })).toBe(false);
});
