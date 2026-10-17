// @vitest-environment happy-dom
// 앱(App)을 가짜 Host 에 붙여 띄우고 키 이벤트를 넣는다: 한글 입력 상태의 ⌘⇧A, 툴 윈도우 다시 열기,
// keymap.toml 덮어쓰기가 Host 범위 config.changed 로 재시작 없이 반영되는 것, 메뉴 accelerator 동기화.

import type { ConfigView, HostInfo, KeymapFile } from "@pitwall/rpc-client";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { DEFAULT_KEYMAP_TOML as DEFAULT_KEYMAP_TOML_FOR_TEST } from "./default-keymap";
import type { KeymapNative } from "./native";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const host = vi.hoisted(() => ({
  keymap: undefined as unknown,
  /** `keymap.get` 의 응답 파일 텍스트. `undefined` 면 번들 기본 키맵과 같은 파일, `null` 이면 오류로 답한다. */
  registryKeymap: undefined as string | null | undefined,
  /** `keymap.get` 응답을 늦출 시간(ms). 설정보다 늦게 오는 경우를 만든다. */
  registryDelayMs: 0,
  keymapGets: 0,
  error: null as ConfigView["error"],
  emitHostEvent: (_topic: string) => {},
  hostSubscriptions: [] as string[][],
}));

vi.mock("../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("./default-keymap");
  return {
    connectHost: async () => {
      const [client, server] = pair();
      const subs = new Map<number, { projectId?: string; topics: string[] }>();
      let next = 1;
      const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const reply = (result: unknown) => send({ id: m.id, result });
        switch (m.method) {
          case "config.get":
            return reply({
              snapshot: {
                files:
                  host.keymap === undefined
                    ? []
                    : [{ id: "keymap", value: host.keymap as never, layers: [] }],
                notices: [],
              },
              error: host.error,
            } satisfies ConfigView);
          case "keymap.get": {
            host.keymapGets++;
            const answer = () => {
              if (host.registryKeymap === null) {
                return send({ id: m.id, error: { code: -32603, message: "internal error" } });
              }
              reply({
                id: "pitwall-default",
                extension: "dev.pitwall.keymap-default",
                kind: "base",
                content: host.registryKeymap ?? DEFAULT_KEYMAP_TOML,
              } satisfies KeymapFile);
            };
            if (host.registryDelayMs > 0) setTimeout(answer, host.registryDelayMs);
            else answer();
            return;
          }
          case "host.info":
            return reply({
              hostId: "local",
              kind: "local",
              version: "0",
              os: "t",
              arch: "t",
            } satisfies HostInfo);
          case "events.subscribe": {
            const id = next++;
            const p = m.params as { projectId?: string; topics: string[] };
            subs.set(id, p);
            if (p.projectId === undefined) host.hostSubscriptions.push(p.topics);
            return reply({ subscriptionId: id });
          }
          case "events.unsubscribe":
            return reply(null);
        }
      });
      host.emitHostEvent = (topic) => {
        for (const [subscriptionId, s] of subs) {
          if (s.projectId !== undefined || !s.topics.includes(topic)) continue;
          send({
            method: "events.event",
            params: { subscriptionId, projectId: null, topic, payload: {} },
          });
        }
      };
      return { transport: client, close: async () => {} };
    },
  };
});

const log = { info: vi.fn(), warn: vi.fn(), debug: vi.fn() };

let root: Root | undefined;
let menuListener: ((action: string) => void) | undefined;
const native = {
  syncMenu: vi.fn<KeymapNative["syncMenu"]>(async () => {}),
  onMenuAction: vi.fn<KeymapNative["onMenuAction"]>((listener) => {
    menuListener = listener;
    return () => {
      if (menuListener === listener) menuListener = undefined;
    };
  }),
  openSettings: vi.fn<KeymapNative["openSettings"]>(async () => {}),
};

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  host.keymap = undefined;
  host.registryKeymap = undefined;
  host.registryDelayMs = 0;
  host.keymapGets = 0;
  host.error = null;
  host.hostSubscriptions = [];
  localStorage.clear();
  native.syncMenu.mockClear();
  native.openSettings.mockClear();
  log.info.mockClear();
  log.warn.mockClear();
});

afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

async function mount(kind: "project" | "settings") {
  const element = document.createElement("div");
  element.style.width = "1280px";
  element.style.height = "800px";
  document.body.append(element);
  const store = createThemeStore({
    system: { current: () => "light", subscribe: () => () => {} },
    storage: { read: () => undefined, write: () => {} },
    theme: parseTheme(defaultTheme),
    apply: () => {},
  });
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <ThemeStoreProvider store={store}>
        <App kind={kind} native={native} log={log} />
      </ThemeStoreProvider>,
    ),
  );
  await settle();
}

const settle = () => act(async () => new Promise((r) => setTimeout(r, 20)));

async function press(code: string, mods: Partial<KeyboardEventInit> = {}, key = code) {
  let prevented = false;
  await act(async () => {
    const event = new KeyboardEvent("keydown", {
      code,
      key,
      bubbles: true,
      cancelable: true,
      ...mods,
    });
    (document.activeElement ?? document.body).dispatchEvent(event);
    prevented = event.defaultPrevented;
    document.body.dispatchEvent(new KeyboardEvent("keyup", { code, key, bubbles: true, ...mods }));
  });
  await settle();
  return prevented;
}

const dialog = () => document.querySelector("[cmdk-dialog]");

it("opens Find Action with ⌘⇧A even when the IME sends a hangul key, but not while composing", async () => {
  await mount("settings");
  expect(await press("KeyA", { metaKey: true, shiftKey: true, isComposing: true }, "ㅁ")).toBe(
    false,
  );
  expect(dialog()).toBeNull();

  expect(await press("KeyA", { metaKey: true, shiftKey: true }, "ㅁ")).toBe(true);
  expect(dialog()).not.toBeNull();
  const items = [...document.querySelectorAll("[cmdk-item]")].map((e) => e.textContent);
  expect(items).toContain("설정⌘,");
});

it("finds an action from a half-composed hangul query and runs it", async () => {
  await mount("settings");
  await press("KeyA", { metaKey: true, shiftKey: true }, "ㅁ");
  const input = document.querySelector<HTMLInputElement>("[cmdk-input]")!;
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    setter.call(input, "설ㅈ");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();
  const items = [...document.querySelectorAll("[cmdk-item]")];
  expect(items.map((e) => e.textContent)).toEqual(["설정⌘,"]);
  await act(async () => (items[0] as HTMLElement).click());
  await settle();
  expect(dialog()).toBeNull();
  expect(native.openSettings).toHaveBeenCalledTimes(1);
});

it("reopens a closed tool window with its shortcut", async () => {
  await mount("project");
  const terminalTab = () =>
    [...document.querySelectorAll<HTMLElement>(".dv-tab")].find((t) =>
      t.textContent?.includes("터미널"),
    );
  // 터미널을 닫는다(탭의 × 와 같은 경로) — 앱은 숨김으로 기록한다.
  await act(async () =>
    terminalTab()!.querySelector<HTMLElement>(".dv-default-tab-action")!.click(),
  );
  await settle();
  expect(terminalTab()).toBeUndefined();

  expect(await press("F12", { altKey: true })).toBe(true);
  expect(terminalTab()).toBeDefined();
  // 다시 연 툴 윈도우가 앞에 보인다.
  expect(document.querySelector('[data-tool-window="terminal"]')).not.toBeNull();
});

it("applies a keymap.toml change without a restart and syncs the menu", async () => {
  await mount("settings");
  expect(host.hostSubscriptions).toContainEqual([
    "config.changed",
    "config.error",
    "project.changed",
  ]);
  expect(native.syncMenu).toHaveBeenLastCalledWith(
    expect.objectContaining({ "action.find": "Super+Shift+KeyA", "settings.open": "Super+Comma" }),
  );

  host.keymap = { bindings: [{ action: "action.find", key: "cmd+k" }] };
  await act(async () => host.emitHostEvent("config.changed"));
  await settle();
  expect(native.syncMenu).toHaveBeenLastCalledWith(
    expect.objectContaining({ "action.find": "Super+KeyK" }),
  );
  expect(await press("KeyA", { metaKey: true, shiftKey: true })).toBe(false);
  expect(dialog()).toBeNull();
  expect(await press("KeyK", { metaKey: true })).toBe(true);
  expect(dialog()).not.toBeNull();
});

it("keeps the previous keymap and shows the error when keymap.toml is malformed", async () => {
  host.keymap = { bindings: [{ action: "action.find", key: "cmd+k" }] };
  await mount("settings");
  vi.spyOn(console, "warn").mockImplementation(() => {});

  host.keymap = { bindings: [{ action: "action.find", key: "cmd+nope" }] };
  await act(async () => host.emitHostEvent("config.changed"));
  await settle();
  expect(document.querySelector("[role=alert]")?.textContent).toContain("cmd+nope");
  expect(await press("KeyK", { metaKey: true })).toBe(true);
  expect(dialog()).not.toBeNull();
});

const fromMenu = async (action: string) => {
  await act(async () => menuListener!(action));
  await settle();
};

it("runs an action once when the same press also arrives from the menu", async () => {
  await mount("settings");
  expect(await press("Comma", { metaKey: true }, ",")).toBe(true);
  await fromMenu("settings.open");
  expect(native.openSettings).toHaveBeenCalledTimes(1);
  // 시간 창이 지난 뒤 메뉴에서 고른 것은 실행한다.
  await act(async () => new Promise((r) => setTimeout(r, 320)));
  await fromMenu("settings.open");
  expect(native.openSettings).toHaveBeenCalledTimes(2);
});

it("does not run shortcuts behind an open Find Action popup", async () => {
  await mount("settings");
  await press("KeyA", { metaKey: true, shiftKey: true });
  expect(dialog()).not.toBeNull();
  expect(await press("Comma", { metaKey: true }, ",")).toBe(false);
  expect(native.openSettings).not.toHaveBeenCalled();
  expect(dialog()).not.toBeNull();
});

it("closes Find Action and runs an action picked from the menu", async () => {
  await mount("settings");
  await press("KeyA", { metaKey: true, shiftKey: true });
  await fromMenu("settings.open");
  expect(dialog()).toBeNull();
  expect(native.openSettings).toHaveBeenCalledTimes(1);
});

it("does not toggle a tool window again while its key is held", async () => {
  await mount("project");
  const collapsedDocks = () => document.querySelectorAll(".dv-edge-collapsed").length;
  await press("Digit1", { metaKey: true }, "1");
  expect(collapsedDocks()).toBe(0);
  // 누르고 있는 동안 오는 반복 keydown 은 토글하지 않는다.
  for (let i = 0; i < 3; i++) await press("Digit1", { metaKey: true, repeat: true }, "1");
  expect(collapsedDocks()).toBe(0);
  // 다시 누르면 토글한다 — 접힘을 이 클래스로 볼 수 있다는 확인이기도 하다.
  await press("Digit1", { metaKey: true }, "1");
  expect(collapsedDocks()).toBe(1);
});

it("uses the last good keymap in a window opened while keymap.toml is malformed", async () => {
  host.keymap = { bindings: [{ action: "action.find", key: "cmd+k" }] };
  await mount("settings");
  act(() => root!.unmount());
  root = undefined;
  document.body.innerHTML = "";

  vi.spyOn(console, "warn").mockImplementation(() => {});
  host.keymap = { bindings: [{ action: "action.find", key: "cmd+nope" }] };
  native.syncMenu.mockClear();
  await mount("settings");
  expect(native.syncMenu).toHaveBeenLastCalledWith(
    expect.objectContaining({ "action.find": "Super+KeyK" }),
  );
  expect(await press("KeyK", { metaKey: true })).toBe(true);
  expect(dialog()).not.toBeNull();
});

it("leaves a default key another action took off the menu", async () => {
  host.keymap = { bindings: [{ action: "settings.open", key: "cmd+shift+a" }] };
  await mount("settings");
  expect(native.syncMenu).toHaveBeenLastCalledWith(
    expect.objectContaining({ "action.find": null, "settings.open": "Super+Shift+KeyA" }),
  );
});

it("takes the default keymap from the extension registry", async () => {
  host.registryKeymap = `schema = 1

[[bindings]]
action = "action.find"
mac = "cmd+j"
win = "ctrl+j"
linux = "ctrl+j"
`;
  await mount("settings");
  expect(native.syncMenu).toHaveBeenLastCalledWith(
    expect.objectContaining({ "action.find": "Super+KeyJ" }),
  );
  expect(await press("KeyJ", { metaKey: true })).toBe(true);
  expect(dialog()).not.toBeNull();
  expect(log.info).toHaveBeenCalledWith(
    "keymap pitwall-default applied from extension dev.pitwall.keymap-default",
    "keymap",
  );
});

it("keeps the bundled default keymap when the registry fails or sends a broken file", async () => {
  for (const reply of [null, "schema = 1\n[[bindings]]\naction = 1\n"]) {
    host.registryKeymap = reply;
    await mount("settings");
    expect(native.syncMenu).toHaveBeenLastCalledWith(
      expect.objectContaining({ "action.find": "Super+Shift+KeyA" }),
    );
    expect(await press("KeyA", { metaKey: true, shiftKey: true })).toBe(true);
    expect(dialog()).not.toBeNull();
    expect(log.warn).toHaveBeenCalledWith(
      expect.stringMatching(
        /^keymap pitwall-default not loaded from the extension registry, using the bundled copy: /,
      ),
      "keymap",
    );
    expect(log.info).not.toHaveBeenCalledWith(expect.stringMatching(/^keymap /), "keymap");
    act(() => root!.unmount());
    root = undefined;
    document.body.innerHTML = "";
    log.warn.mockClear();
  }
});

it("applies a registry default keymap that arrives after a malformed keymap.toml", async () => {
  host.keymap = { bindings: [{ action: "action.find", key: "cmd+nope" }] };
  host.registryKeymap = DEFAULT_KEYMAP_TOML_FOR_TEST.replace('mac = "cmd+,"', 'mac = "cmd+j"');
  host.registryDelayMs = 50;
  vi.spyOn(console, "warn").mockImplementation(() => {});
  await mount("settings");
  await act(async () => new Promise((r) => setTimeout(r, 80)));
  expect(native.syncMenu).toHaveBeenLastCalledWith(
    expect.objectContaining({ "settings.open": "Super+KeyJ", "action.find": "Super+Shift+KeyA" }),
  );
  expect(document.querySelector("[role=alert]")?.textContent).toContain("cmd+nope");
});

it("asks the registry for the default keymap once, not on every focus", async () => {
  await mount("settings");
  expect(host.keymapGets).toBe(1);
  await act(async () => {
    // TanStack Query 의 focusManager 는 window 의 visibilitychange 를 듣는다.
    window.dispatchEvent(new Event("visibilitychange"));
  });
  await settle();
  expect(host.keymapGets).toBe(1);
  expect(log.warn).not.toHaveBeenCalled();
});
