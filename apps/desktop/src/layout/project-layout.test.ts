// @vitest-environment happy-dom
import { createDockview, type DockviewApi } from "dockview-react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  buildDefaultLayout,
  LAYOUT_STORAGE_KEY,
  LAYOUT_VERSION,
  PANELS,
  persistLayout,
  restoreLayout,
  saveLayout,
  showPanel,
} from "./project-layout";

// React 가 아니라 dockview-core 로 같은 api 를 만든다. 패널 내용은 빈 요소면 된다.
function dockview(): DockviewApi {
  const element = document.createElement("div");
  document.body.append(element);
  const api = createDockview(element, {
    createComponent: () => ({ element: document.createElement("div"), init: () => {} }),
  });
  api.layout(1280, 760);
  return api;
}

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

  it("restores a closed panel as closed, and the tool window bar reopens it at home", () => {
    const first = dockview();
    buildDefaultLayout(first);
    first.getPanel("commit")!.api.close();
    const storage = memoryStorage();
    saveLayout(first, storage);

    const second = dockview();
    restoreLayout(second, storage);
    expect(second.getPanel("commit")).toBeUndefined();
    showPanel(second, "commit");
    expect(placement(second).commit).toBe("left");
    expect(edge(second, "left").activePanel?.id).toBe("commit");
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
  it("saves once after a burst of layout changes, and flushes a pending save on pagehide", async () => {
    vi.useFakeTimers();
    const api = dockview();
    buildDefaultLayout(api);
    const storage = memoryStorage();
    const setItem = vi.spyOn(storage, "setItem");
    const win = new EventTarget();
    const persisted = persistLayout(api, storage, { delayMs: 300, win });

    api.getPanel("terminal")!.api.moveTo({ group: edge(api, "right") });
    api.getPanel("git")!.api.moveTo({ group: edge(api, "left") });
    // dockview 는 변경을 마이크로태스크로 모아 onDidLayoutChange 를 한 번 낸다.
    await Promise.resolve();
    vi.advanceTimersByTime(299);
    expect(setItem).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(setItem).toHaveBeenCalledTimes(1);

    api.getPanel("run")!.api.moveTo({ group: edge(api, "right") });
    await Promise.resolve();
    win.dispatchEvent(new Event("pagehide"));
    expect(setItem).toHaveBeenCalledTimes(2);

    const restored = dockview();
    restoreLayout(restored, storage);
    expect(placement(restored)).toMatchObject({ terminal: "right", git: "left", run: "right" });
    persisted.dispose();
  });
});
