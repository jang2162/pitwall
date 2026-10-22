// @vitest-environment happy-dom
// 창의 연결 대상(App.tsx 의 `windowHostTarget`): 프로젝트 창만 자기 프로젝트로 연결하고 — 그 연결만 그 프로젝트의 상태를 쓴다
// (`project.state.set`) — 나머지 창은 Host 를 직접 고른다.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../extensions/theme-default/themes/default.json";
import { App, windowHostTarget } from "./App";
import type { KeymapNative } from "./keymap/native";
import type { HostTarget } from "./tauri-transport";
import { createThemeStore } from "./theme/theme-store";
import { ThemeStoreProvider } from "./theme/theme-switcher";
import { parseTheme } from "./theme/tokens";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const targets = vi.hoisted(() => [] as HostTarget[]);

// 답하지 않는 Host. 창이 어디로 연결하는지만 본다.
vi.mock("./tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  return {
    connectHost: async (target: HostTarget) => {
      targets.push(target);
      const [client] = pair();
      return { transport: client, close: async () => {} };
    },
  };
});

const native: KeymapNative = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
  closeWindow: async () => {},
};

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(console, "info").mockImplementation(() => {});
  // 답하지 않는 Host 를 닫을 때 진행 중이던 호출이 경고를 남긴다.
  vi.spyOn(console, "warn").mockImplementation(() => {});
  targets.length = 0;
});

afterEach(() => {
  document.body.innerHTML = "";
  window.history.replaceState(null, "", "/");
  vi.restoreAllMocks();
});

it("connects a project window by its project and every other window by the host", () => {
  expect(windowHostTarget("project", "?window=project&project=p1")).toEqual({ projectId: "p1" });
  // 프로젝트를 모르는 프로젝트 창(쿼리 없음)은 Host 로 — 상태를 쓰지 못할 뿐 창은 뜬다.
  expect(windowHostTarget("project", "?window=project")).toEqual({ hostId: "local" });
  for (const kind of ["welcome", "settings", "file"] as const) {
    expect(windowHostTarget(kind, `?window=${kind}&project=p1&path=%2Fx%2Fa.txt`)).toEqual({
      hostId: "local",
    });
  }
});

it("opens the project window's connection to its project", async () => {
  window.history.replaceState(null, "", "/?window=project&project=p1");
  const element = document.createElement("div");
  document.body.append(element);
  const store = createThemeStore({
    system: { current: () => "light", subscribe: () => () => {} },
    storage: { read: () => undefined, write: () => {} },
    theme: parseTheme(defaultTheme),
    apply: () => {},
  });
  const root = createRoot(element);
  await act(async () =>
    root.render(
      <ThemeStoreProvider store={store}>
        <App kind="project" native={native} />
      </ThemeStoreProvider>,
    ),
  );
  await act(async () => new Promise((r) => setTimeout(r, 20)));
  act(() => root.unmount());
  expect(targets).toEqual([{ projectId: "p1" }]);
});
