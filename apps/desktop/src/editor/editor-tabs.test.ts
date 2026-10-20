// @vitest-environment happy-dom
import { createDockview, type DockviewApi } from "dockview-react";
import { beforeEach, describe, expect, it } from "vitest";

import {
  closeOtherTabs,
  EDITOR_PANEL,
  editorPanels,
  findTab,
  keepTab,
  openTab,
  splitRight,
  tabParams,
  targetGroup,
  toggleEditorMaximized,
  togglePinned,
  trackRecentTabs,
} from "./editor-tabs";

function dockview(): DockviewApi {
  const element = document.createElement("div");
  document.body.append(element);
  const api = createDockview(element, {
    createComponent: () => ({ element: document.createElement("div"), init: () => {} }),
  });
  api.layout(1280, 760);
  // 앱처럼 왼쪽 · 하단 dock 에 툴 윈도우를 둔다.
  for (const [dock, id] of [
    ["left", "project"],
    ["bottom", "terminal"],
  ] as const) {
    api.addEdgeGroup(dock, { id: `dock-${dock}`, initialSize: 200 });
    api.addPanel({ id, component: "toolWindow", position: { referenceGroup: `dock-${dock}` } });
  }
  return api;
}

const settle = () => new Promise<void>((resolve) => queueMicrotask(resolve));

/** 그룹마다 탭(파일 이름 + 미리보기 표시). */
function groups(api: DockviewApi): string[][] {
  return api.groups
    .filter((g) => g.api.location.type === "grid")
    .map((g) =>
      g.panels.map((p) => {
        const { path, preview, pinned } = tabParams(p);
        return `${path.split("/").pop()}${preview ? "*" : ""}${pinned ? "!" : ""}`;
      }),
    );
}

beforeEach(() => {
  document.body.innerHTML = "";
});

describe("openTab", () => {
  it("opens into the centre even when a tool window dock is active", () => {
    const api = dockview();
    api.getPanel("project")!.api.setActive();
    const { panel } = openTab(api, { path: "/p/a.rs" });
    expect(panel.group.api.location.type).toBe("grid");
    expect(panel.api.component).toBe(EDITOR_PANEL);
    expect(panel.title).toBe("a.rs");
    expect(targetGroup(api).id).toBe(panel.group.id);
  });

  it("activates the open tab instead of opening a second one", () => {
    const api = dockview();
    const first = openTab(api, { path: "/p/a.rs" });
    openTab(api, { path: "/p/b.rs" });
    const again = openTab(api, { path: "/p/a.rs" });
    expect(again).toEqual({ panel: first.panel, created: false });
    expect(api.activePanel?.id).toBe(first.panel.id);
    expect(groups(api)).toEqual([["a.rs", "b.rs"]]);
  });

  it("lets the next preview take the preview tab's place, and keeps it when opened for good", () => {
    const api = dockview();
    openTab(api, { path: "/p/a.rs" });
    openTab(api, { path: "/p/b.rs", preview: true });
    openTab(api, { path: "/p/c.rs" });
    expect(groups(api)).toEqual([["a.rs", "b.rs*", "c.rs"]]);
    openTab(api, { path: "/p/d.rs", preview: true });
    expect(groups(api)).toEqual([["a.rs", "d.rs*", "c.rs"]]);
    // 같은 파일을 고정으로 다시 열면 보통 탭이 된다. 미리보기로 다시 열면 그대로다.
    openTab(api, { path: "/p/d.rs", preview: true });
    expect(groups(api)).toEqual([["a.rs", "d.rs*", "c.rs"]]);
    openTab(api, { path: "/p/d.rs" });
    expect(groups(api)).toEqual([["a.rs", "d.rs", "c.rs"]]);
    openTab(api, { path: "/p/e.rs", preview: true });
    expect(groups(api)).toEqual([["a.rs", "d.rs", "c.rs", "e.rs*"]]);
  });

  it("replaces a lone preview tab without losing its group", () => {
    const api = dockview();
    openTab(api, { path: "/p/a.rs", preview: true });
    openTab(api, { path: "/p/b.rs", preview: true });
    expect(groups(api)).toEqual([["b.rs*"]]);
  });

  it("keeps a preview per group", () => {
    const api = dockview();
    const { panel } = openTab(api, { path: "/p/a.rs", preview: true });
    const right = splitRight(api, panel);
    openTab(api, { path: "/p/b.rs", preview: true });
    expect(right.group.api.isActive).toBe(true);
    expect(groups(api)).toEqual([["a.rs*"], ["a.rs", "b.rs*"]]);
  });
});

describe("tab operations", () => {
  it("closes the other unpinned tabs of the group only", () => {
    const api = dockview();
    const a = openTab(api, { path: "/p/a.rs" }).panel;
    openTab(api, { path: "/p/b.rs" });
    const c = openTab(api, { path: "/p/c.rs" }).panel;
    splitRight(api, c);
    togglePinned(a);
    closeOtherTabs(c);
    expect(groups(api)).toEqual([["a.rs!", "c.rs"], ["c.rs"]]);
    expect(api.getPanel("project")).toBeDefined();
  });

  it("pins a preview tab for good, and unpins it", () => {
    const api = dockview();
    const { panel } = openTab(api, { path: "/p/a.rs", preview: true });
    togglePinned(panel);
    expect(tabParams(panel)).toEqual({ path: "/p/a.rs", preview: false, pinned: true });
    togglePinned(panel);
    expect(tabParams(panel).pinned).toBe(false);
    keepTab(panel);
    expect(tabParams(panel).preview).toBe(false);
  });

  it("splits a tab into a new group on the right showing the same file", () => {
    const api = dockview();
    const { panel } = openTab(api, { path: "/p/a.rs" });
    const right = splitRight(api, panel);
    expect(right.id).not.toBe(panel.id);
    expect(right.group.id).not.toBe(panel.group.id);
    expect(groups(api)).toEqual([["a.rs"], ["a.rs"]]);
    expect(api.activePanel?.id).toBe(right.id);
    // 같은 파일을 다시 열면 지금 그룹의 탭을 고른다.
    expect(findTab(api, "/p/a.rs", { group: panel.group })?.id).toBe(panel.id);
    expect(openTab(api, { path: "/p/a.rs" }).panel.id).toBe(right.id);
  });

  it("toggles the maximized editor by folding only the docks that were open", () => {
    const api = dockview();
    openTab(api, { path: "/p/a.rs" });
    api.getEdgeGroup("bottom")!.collapse();
    toggleEditorMaximized(api);
    expect(api.getEdgeGroup("left")!.isCollapsed()).toBe(true);
    expect(api.getEdgeGroup("bottom")!.isCollapsed()).toBe(true);
    toggleEditorMaximized(api);
    expect(api.getEdgeGroup("left")!.isCollapsed()).toBe(false);
    // 최대화 전에 접혀 있던 dock 은 그대로 접혀 있다.
    expect(api.getEdgeGroup("bottom")!.isCollapsed()).toBe(true);
  });
});

it("remembers editor tabs in the order they were last used and forgets closed ones", async () => {
  const api = dockview();
  const recent = trackRecentTabs(api);
  const a = openTab(api, { path: "/p/a.rs" }).panel;
  const b = openTab(api, { path: "/p/b.rs" }).panel;
  const c = openTab(api, { path: "/p/c.rs" }).panel;
  api.getPanel("terminal")!.api.setActive();
  a.api.setActive();
  expect(recent.order()).toEqual([a.id, c.id, b.id]);
  // 그룹 사이로 옮겨도 잊지 않고 자리도 그대로다.
  const right = splitRight(api, a);
  a.api.setActive();
  expect(recent.order()).toEqual([a.id, right.id, c.id, b.id]);
  c.api.moveTo({ group: right.group, skipSetActive: true });
  await settle();
  expect(recent.order()).toEqual([a.id, right.id, c.id, b.id]);
  b.api.close();
  expect(recent.order()).not.toContain(b.id);
  expect(editorPanels(api).map((p) => p.id)).not.toContain("terminal");
  recent.dispose();
});
