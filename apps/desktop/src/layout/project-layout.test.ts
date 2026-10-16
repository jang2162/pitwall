// @vitest-environment happy-dom
import { createDockview, type DockviewApi } from "dockview-react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  buildDefaultLayout,
  collapseAllDocks,
  EDITOR_TAB,
  hiddenPanels,
  keepEditor,
  LAYOUT_STORAGE_KEY,
  LAYOUT_VERSION,
  PANELS,
  persistLayout,
  restoreLayout,
  saveLayout,
  showPanel,
  toggleToolWindow,
  trackHiddenPanels,
} from "./project-layout";

// React 가 아니라 dockview-core 로 같은 api 를 만든다. 패널 내용은 빈 요소면 된다.
function dockview(): DockviewApi {
  const element = document.createElement("div");
  document.body.append(element);
  const api = createDockview(element, {
    createComponent: () => ({ element: document.createElement("div"), init: () => {} }),
  });
  api.layout(1280, 760);
  // 앱(project-window.tsx 의 ProjectDock)처럼 닫기를 숨김으로 기록한다.
  trackHiddenPanels(api);
  return api;
}

/** 닫기가 숨김으로 기록되기를 기다린다(trackHiddenPanels 는 흐름이 끝난 뒤에 본다). */
const settle = () => new Promise<void>((resolve) => queueMicrotask(resolve));

function memoryStorage(initial?: string) {
  const items = new Map<string, string>();
  if (initial !== undefined) items.set(LAYOUT_STORAGE_KEY, initial);
  return {
    items,
    getItem: (key: string) => items.get(key) ?? null,
    setItem: (key: string, value: string) => void items.set(key, value),
    removeItem: (key: string) => void items.delete(key),
  };
}

/** 패널 id → 그 패널이 든 곳(edge group 의 위치 또는 "editor"). */
function placement(api: DockviewApi): Record<string, string> {
  const where: Record<string, string> = {};
  for (const panel of api.panels) {
    const dock = (["left", "right", "bottom"] as const).find(
      (d) => api.getEdgeGroup(d)?.id === panel.group.id,
    );
    where[panel.id] = dock ?? "editor";
  }
  return where;
}

/** edge group 의 그룹. `getEdgeGroup` 은 그룹이 아니라 그룹의 api 를 준다. */
function edge(api: DockviewApi, dock: "left" | "right" | "bottom") {
  const id = api.getEdgeGroup(dock)!.id;
  return api.groups.find((g) => g.id === id)!;
}

const defaultPlacement = Object.fromEntries(PANELS.map((p) => [p.id, p.home]));

beforeEach(() => {
  document.body.innerHTML = "";
  vi.spyOn(console, "warn").mockImplementation(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe("default layout", () => {
  it("puts every tool window at its home dock and the editor area in the centre", () => {
    const api = dockview();
    buildDefaultLayout(api);
    expect(placement(api)).toEqual(defaultPlacement);
    expect(edge(api, "left").activePanel?.id).toBe("project");
    expect(edge(api, "bottom").activePanel?.id).toBe("run");
    expect(edge(api, "right").activePanel?.id).toBe("notifications");
    expect(api.activePanel?.id).toBe("editor");
  });
});

describe("save and restore", () => {
  it("opens the default layout when nothing is saved", () => {
    const api = dockview();
    expect(restoreLayout(api, memoryStorage())).toBe("default");
    expect(placement(api)).toEqual(defaultPlacement);
  });

  it("restores a moved panel in a new dockview", () => {
    const first = dockview();
    buildDefaultLayout(first);
    // 터미널을 하단에서 오른쪽 툴 윈도우로 옮긴다.
    first.getPanel("terminal")!.api.moveTo({ group: edge(first, "right") });
    const storage = memoryStorage();
    saveLayout(first, storage);

    const second = dockview();
    expect(restoreLayout(second, storage)).toBe("restored");
    expect(placement(second).terminal).toBe("right");
    expect(placement(second)).toEqual({ ...defaultPlacement, terminal: "right" });
  });

  it("keeps a closed tool window hidden after restore and reopens it where it was closed", async () => {
    const first = dockview();
    buildDefaultLayout(first);
    first.getPanel("structure")!.api.setActive();
    first.getPanel("terminal")!.api.moveTo({ group: edge(first, "right") });
    first.getPanel("commit")!.api.close();
    first.getPanel("terminal")!.api.close();
    await settle();
    const storage = memoryStorage();
    saveLayout(first, storage);

    const second = dockview();
    restoreLayout(second, storage);
    const { commit: _commit, terminal: _terminal, ...visible } = defaultPlacement;
    expect(placement(second)).toEqual(visible);
    expect([...hiddenPanels(second).entries()]).toEqual([
      ["commit", "left"],
      ["terminal", "right"],
    ]);
    expect(edge(second, "left").activePanel?.id).toBe("structure");

    showPanel(second, "terminal");
    expect(placement(second).terminal).toBe("right");
    expect(hiddenPanels(second).has("terminal")).toBe(false);
  });

  it("reopens a tool window closed during the session where it was", async () => {
    const api = dockview();
    buildDefaultLayout(api);
    api.getPanel("commit")!.api.close();
    await settle();
    showPanel(api, "commit");
    expect(placement(api).commit).toBe("left");
    expect(edge(api, "left").activePanel?.id).toBe("commit");
  });

  it("does not take a moved tool window for a hidden one", async () => {
    const api = dockview();
    buildDefaultLayout(api);
    api.getPanel("terminal")!.api.moveTo({ group: edge(api, "right") });
    await settle();
    expect(hiddenPanels(api).size).toBe(0);
  });

  it("ignores unknown entries in the saved hidden list", () => {
    const first = dockview();
    buildDefaultLayout(first);
    const storage = memoryStorage(
      JSON.stringify({
        version: LAYOUT_VERSION,
        layout: first.toJSON(),
        hidden: { nope: "left", editor: "left", commit: "top", project: "right" },
      }),
    );
    const second = dockview();
    expect(restoreLayout(second, storage)).toBe("restored");
    // project 는 저장본의 레이아웃에도 있으므로 보이는 쪽을 믿는다.
    expect(hiddenPanels(second).size).toBe(0);
    expect(placement(second)).toEqual(defaultPlacement);
  });

  it("puts a tool window missing from the save at its home", () => {
    const first = dockview();
    buildDefaultLayout(first);
    const layout = first.toJSON();
    // 툴 윈도우를 더하기 전에 만든 저장본처럼, 하단의 「쿼리 결과」를 저장본에서 뺀다.
    delete layout.panels.queryResults;
    const bottom = layout.edgeGroups!.bottom!.group as { views: string[] };
    bottom.views = bottom.views.filter((id) => id !== "queryResults");
    const storage = memoryStorage(JSON.stringify({ version: LAYOUT_VERSION, layout }));

    const second = dockview();
    expect(restoreLayout(second, storage)).toBe("restored");
    expect(placement(second).queryResults).toBe("bottom");
  });

  it("leaves a moved tool window where it was moved", async () => {
    const first = dockview();
    buildDefaultLayout(first);
    first.getPanel("terminal")!.api.moveTo({ group: edge(first, "right") });
    first.getPanel("commit")!.api.close();
    await settle();
    const storage = memoryStorage();
    saveLayout(first, storage);

    const second = dockview();
    restoreLayout(second, storage);
    const { commit: _commit, ...rest } = defaultPlacement;
    expect(placement(second)).toEqual({ ...rest, terminal: "right" });
    expect(second.panels.filter((p) => p.id === "terminal")).toHaveLength(1);
  });
});

describe("collapsed tool window dock", () => {
  // 왼쪽 툴 윈도우를 모두 닫으면 dockview 가 비게 된 edge 그룹을 접는다.
  async function emptyLeft() {
    const api = dockview();
    buildDefaultLayout(api);
    for (const panel of PANELS.filter((p) => p.home === "left"))
      api.getPanel(panel.id)!.api.close();
    await settle();
    expect(api.getEdgeGroup("left")!.isCollapsed()).toBe(true);
    return api;
  }

  it("is expanded when a hidden tool window is reopened into it after restore", async () => {
    const first = await emptyLeft();
    const storage = memoryStorage();
    saveLayout(first, storage);

    const second = dockview();
    restoreLayout(second, storage);
    expect(placement(second).project).toBeUndefined();
    showPanel(second, "project");
    expect(placement(second).project).toBe("left");
    expect(second.getEdgeGroup("left")!.isCollapsed()).toBe(false);
  });

  it("is expanded when the tool window bar puts a panel into it", async () => {
    const api = await emptyLeft();
    showPanel(api, "project");
    expect(api.getEdgeGroup("left")!.isCollapsed()).toBe(false);
  });

  it("is expanded when the tool window bar picks a panel already in it", () => {
    const api = dockview();
    buildDefaultLayout(api);
    api.getEdgeGroup("left")!.collapse();
    showPanel(api, "commit");
    expect(api.getEdgeGroup("left")!.isCollapsed()).toBe(false);
  });
});

describe("tool window actions", () => {
  it("shows a tool window, and collapses its dock when it is already the active one", () => {
    const api = dockview();
    buildDefaultLayout(api);
    toggleToolWindow(api, "commit");
    expect(api.activePanel?.id).toBe("commit");
    expect(api.getEdgeGroup("left")!.isCollapsed()).toBe(false);

    toggleToolWindow(api, "commit");
    expect(api.getEdgeGroup("left")!.isCollapsed()).toBe(true);
    expect(placement(api).commit).toBe("left");

    toggleToolWindow(api, "commit");
    expect(api.getEdgeGroup("left")!.isCollapsed()).toBe(false);
  });

  it("reopens a hidden tool window", async () => {
    const api = dockview();
    buildDefaultLayout(api);
    api.getPanel("terminal")!.api.close();
    await settle();
    toggleToolWindow(api, "terminal");
    expect(placement(api).terminal).toBe("bottom");
    expect(api.activePanel?.id).toBe("terminal");
  });

  it("collapses every dock and keeps the panels", () => {
    const api = dockview();
    buildDefaultLayout(api);
    collapseAllDocks(api);
    for (const dock of ["left", "right", "bottom"] as const) {
      expect(api.getEdgeGroup(dock)!.isCollapsed()).toBe(true);
    }
    expect(placement(api)).toEqual(defaultPlacement);
  });
});

describe("broken saved layout", () => {
  const valid = () => {
    const api = dockview();
    buildDefaultLayout(api);
    return api.toJSON();
  };
  const cases: [string, () => string][] = [
    ["not JSON", () => "{not json"],
    ["not an object", () => "42"],
    ["another version", () => JSON.stringify({ version: LAYOUT_VERSION + 1, layout: valid() })],
    ["no layout", () => JSON.stringify({ version: LAYOUT_VERSION })],
    [
      "rejected by dockview",
      () => JSON.stringify({ version: LAYOUT_VERSION, layout: { panels: {} } }),
    ],
    [
      "an unknown panel",
      () => {
        const layout = valid();
        layout.panels.gone = { id: "gone", contentComponent: "toolWindow", title: "x" };
        return JSON.stringify({ version: LAYOUT_VERSION, layout });
      },
    ],
    [
      "a panel with another component",
      () => {
        const layout = valid();
        layout.panels.project!.contentComponent = "editorArea";
        return JSON.stringify({ version: LAYOUT_VERSION, layout });
      },
    ],
  ];

  it.each(cases)("falls back to the default layout and drops a save that is %s", (_, saved) => {
    const storage = memoryStorage(saved());
    const api = dockview();
    expect(restoreLayout(api, storage)).toBe("recovered");
    expect(placement(api)).toEqual(defaultPlacement);
    expect(storage.items.has(LAYOUT_STORAGE_KEY)).toBe(false);
  });
});

describe("persistLayout", () => {
  // dockview 는 같은 흐름의 변경을 마이크로태스크로 모아 onDidLayoutChange 를 한 번 낸다.
  const settle = () => Promise.resolve();

  it("saves within one frame of the first change, without later changes pushing it back", async () => {
    vi.useFakeTimers();
    const api = dockview();
    buildDefaultLayout(api);
    const storage = memoryStorage();
    const setItem = vi.spyOn(storage, "setItem");
    const persisted = persistLayout(api, storage, { frameMs: 16, win: new EventTarget() });

    api.getPanel("terminal")!.api.moveTo({ group: edge(api, "right") });
    await settle();
    vi.advanceTimersByTime(10);
    // 크기 조절처럼 이어지는 변경. 저장 시점을 미루지 않는다.
    api.getPanel("git")!.api.moveTo({ group: edge(api, "left") });
    await settle();
    vi.advanceTimersByTime(5);
    expect(setItem).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(setItem).toHaveBeenCalledTimes(1);

    const restored = dockview();
    restoreLayout(restored, storage);
    expect(placement(restored)).toMatchObject({ terminal: "right", git: "left" });
    persisted.dispose();
  });

  it("flushes a pending save on pagehide", async () => {
    vi.useFakeTimers();
    const api = dockview();
    buildDefaultLayout(api);
    const storage = memoryStorage();
    const setItem = vi.spyOn(storage, "setItem");
    const win = new EventTarget();
    const persisted = persistLayout(api, storage, { frameMs: 16, win });

    api.getPanel("run")!.api.moveTo({ group: edge(api, "right") });
    await settle();
    win.dispatchEvent(new Event("pagehide"));
    expect(setItem).toHaveBeenCalledTimes(1);
    const restored = dockview();
    restoreLayout(restored, storage);
    expect(placement(restored).run).toBe("right");
    persisted.dispose();
  });
});

describe("editor area", () => {
  it("puts the editor back in the centre when a save has none", () => {
    const first = dockview();
    buildDefaultLayout(first);
    first.getPanel("editor")!.api.close();
    expect(first.getPanel("editor")).toBeUndefined();
    const storage = memoryStorage();
    saveLayout(first, storage);

    const second = dockview();
    expect(restoreLayout(second, storage)).toBe("restored");
    expect(placement(second)).toEqual(defaultPlacement);
  });

  it("gives the editor its no-close tab even in a save made before the tab existed", () => {
    const first = dockview();
    buildDefaultLayout(first);
    const layout = first.toJSON();
    delete layout.panels.editor!.tabComponent;
    layout.panels.project!.tabComponent = EDITOR_TAB;
    const storage = memoryStorage(JSON.stringify({ version: LAYOUT_VERSION, layout }));

    const second = dockview();
    restoreLayout(second, storage);
    const restored = second.toJSON().panels;
    expect(restored.editor!.tabComponent).toBe(EDITOR_TAB);
    expect(restored.project!.tabComponent).toBeUndefined();
  });

  it("puts a removed editor back in the centre, and leaves a moved one alone", async () => {
    const api = dockview();
    buildDefaultLayout(api);
    const guard = keepEditor(api);

    api.getPanel("editor")!.api.close();
    await Promise.resolve();
    expect(placement(api).editor).toBe("editor");

    api.getPanel("editor")!.api.moveTo({ group: edge(api, "bottom") });
    await Promise.resolve();
    expect(api.panels.filter((p) => p.id === "editor")).toHaveLength(1);
    expect(placement(api).editor).toBe("bottom");
    guard.dispose();
  });
});
