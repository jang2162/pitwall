// @vitest-environment happy-dom
import type { DockviewApi } from "dockview-react";
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { LAYOUT_STORAGE_KEY } from "./project-layout";
import { ProjectDock } from "./project-window";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(console, "info").mockImplementation(() => {});
});

afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
  vi.restoreAllMocks();
});

function memoryStorage() {
  const items = new Map<string, string>();
  return {
    items,
    getItem: (key: string) => items.get(key) ?? null,
    setItem: (key: string, value: string) => void items.set(key, value),
    removeItem: (key: string) => void items.delete(key),
  };
}

async function mount(storage: ReturnType<typeof memoryStorage>) {
  const element = document.createElement("div");
  element.style.width = "1280px";
  element.style.height = "760px";
  document.body.append(element);
  let api: DockviewApi | undefined;
  root = createRoot(element);
  // 앱처럼 StrictMode 로 렌더한다. StrictMode 는 effect 를 한 번 해제했다 다시 붙인다.
  await act(async () =>
    root!.render(
      <StrictMode>
        <ProjectDock storage={storage} onApi={(a) => (api = a)} />
      </StrictMode>,
    ),
  );
  return api!;
}

it("keeps saving layout changes after StrictMode remounts effects, and restores them", async () => {
  const storage = memoryStorage();
  const api = await mount(storage);
  expect(api.getPanel("commit")).toBeDefined();

  await act(async () => api.getPanel("commit")!.api.close());
  await vi.waitFor(() => expect(storage.items.has(LAYOUT_STORAGE_KEY)).toBe(true), {
    timeout: 2000,
  });

  act(() => root!.unmount());
  root = undefined;
  const restored = await mount(storage);
  expect(restored.getPanel("commit")).toBeUndefined();
  expect(restored.getPanel("project")).toBeDefined();
});

function tabOf(title: string) {
  const tab = [...document.querySelectorAll<HTMLElement>("[role=tab]")].find(
    (el) => el.textContent === title,
  );
  if (!tab) throw new Error(`탭 ${title} 이 없다`);
  return tab;
}

it("draws the editor tab without a close button, unlike tool window tabs", async () => {
  await mount(memoryStorage());
  expect(tabOf("에디터").querySelector(".dv-default-tab-action")).toBeNull();
  expect(tabOf("프로젝트").querySelector(".dv-default-tab-action")).not.toBeNull();
});

it("keeps the editor when its focused tab gets Delete", async () => {
  const api = await mount(memoryStorage());
  const tab = tabOf("에디터");
  await act(async () => {
    tab.focus();
    tab.dispatchEvent(new KeyboardEvent("keydown", { key: "Delete", bubbles: true }));
  });
  expect(api.getPanel("editor")).toBeDefined();
  expect(api.groups.find((g) => g.panels.some((p) => p.id === "editor"))?.api.location.type).toBe(
    "grid",
  );
});
