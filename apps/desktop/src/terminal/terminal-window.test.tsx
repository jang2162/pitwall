// @vitest-environment happy-dom
// 프로젝트 창(App kind="project")을 가짜 Host 에 붙여 터미널 툴 윈도우를 띄운다: ⌥F12 로 탭이 열리고 활성 워크트리로 셸을 띄우는 것,
// 터미널 포커스의 키 경계(Ctrl+C 는 셸로, ⌘W 는 터미널 탭 닫기), 한글 조합 입력이 셸에 한 번만 가는 것, 외관을 바꾸면 터미널 색이
// 따라 바뀌는 것, 셸이 끝나면 탭에 표시되는 것, 파일 드롭이 경로로 붙는 것.

import type { ConfigView, HostInfo, KeymapFile } from "@pitwall/rpc-client";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import type { KeymapNative } from "../keymap/native";
import { activeWorktreeStore } from "../open-with/active-worktree";
import { createThemeStore, type ThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { windowTerminals } from "./terminal-tabs";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const host = vi.hoisted(() => ({
  creates: [] as { streamId: number; params: Record<string, unknown> }[],
  inputs: [] as string[],
  cancelled: [] as number[],
  send: (_m: object) => {},
  drop: undefined as
    | ((e: {
        payload: { type: string; paths: string[]; position: { x: number; y: number } };
      }) => void)
    | undefined,
}));

vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: async (handler: typeof host.drop) => {
      host.drop = handler;
      return () => {
        host.drop = undefined;
      };
    },
  }),
}));

vi.mock("../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../keymap/default-keymap");
  const { i18nState } = await import("../i18n/testing");
  return {
    connectHost: async () => {
      const [client, server] = pair();
      let next = 1;
      const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
      host.send = send;
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const reply = (result: unknown) => send({ id: m.id, result });
        switch (m.method) {
          case "i18n.get":
            return reply(i18nState("ko"));
          case "config.get":
            return reply({
              snapshot: { files: [], notices: [] },
              error: null,
            } satisfies ConfigView);
          case "keymap.get":
            return reply({
              id: "pitwall-default",
              extension: "dev.pitwall.keymap-default",
              kind: "base",
              content: DEFAULT_KEYMAP_TOML,
            } satisfies KeymapFile);
          case "host.info":
            return reply({
              hostId: "local",
              kind: "local",
              version: "0",
              os: "t",
              arch: "t",
            } satisfies HostInfo);
          case "project.get":
            return reply({ id: "p1", name: "demo", roots: ["/main"] });
          case "project.trust":
            return reply({ trusted: true, ownerId: "p1", asked: true });
          case "task.list":
            return reply([]);
          case "events.subscribe":
            return reply({ subscriptionId: next++ });
          case "terminal.create": {
            const streamId = next++;
            host.creates.push({ streamId, params: m.params });
            return reply({ streamId });
          }
          case "terminal.input":
            host.inputs.push((m.params as { data: string }).data);
            return reply(null);
          case "terminal.resize":
            return reply(null);
          case "stream.cancel":
            host.cancelled.push((m.params as { streamId: number }).streamId);
            return;
          case "stream.credit":
            return;
          default:
            return m.id === undefined
              ? undefined
              : send({ id: m.id, error: { code: -32601, message: `no ${m.method}` } });
        }
      });
      return { transport: client, close: async () => {} };
    },
  };
});

let menuAction: ((action: string) => void) | undefined;
const native: KeymapNative = {
  syncMenu: async () => {},
  onMenuAction: (handler) => {
    menuAction = handler;
    return () => {
      menuAction = undefined;
    };
  },
  openSettings: async () => {},
  closeWindow: async () => {},
};

let root: Root | undefined;
let store: ThemeStore;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  vi.spyOn(console, "warn").mockImplementation(() => {});
  host.creates = [];
  host.inputs = [];
  host.cancelled = [];
  localStorage.clear();
  window.history.replaceState(null, "", "/?window=project&project=p1");
  activeWorktreeStore.setState({ top: "/wt" });
});

afterEach(() => {
  act(() => windowTerminals().disposeAll());
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  activeWorktreeStore.setState({ top: undefined });
  Object.defineProperty(window, "devicePixelRatio", { configurable: true, value: 1 });
  vi.restoreAllMocks();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 30)));

async function mount() {
  const element = document.createElement("div");
  element.style.width = "1280px";
  element.style.height = "800px";
  document.body.append(element);
  store = createThemeStore({
    system: { current: () => "light", subscribe: () => () => {} },
    storage: { read: () => undefined, write: () => {} },
    theme: parseTheme(defaultTheme),
    apply: () => {},
  });
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <ThemeStoreProvider store={store}>
        <App kind="project" native={native} />
      </ThemeStoreProvider>,
    ),
  );
  await settle();
}

async function keydown(target: Element, code: string, init: Partial<KeyboardEventInit> = {}) {
  await act(async () => {
    target.dispatchEvent(
      new KeyboardEvent("keydown", { code, key: code, bubbles: true, cancelable: true, ...init }),
    );
  });
  await settle();
}

function chunk(streamId: number, data: object) {
  host.send({ method: "stream.chunk", params: { streamId, data } });
}

const b64 = (text: string) => btoa(String.fromCharCode(...new TextEncoder().encode(text)));
const textarea = () => document.querySelector<HTMLTextAreaElement>(".xterm-helper-textarea")!;
const tabs = () => [...document.querySelectorAll<HTMLElement>("[data-testid=terminal-tab]")];

/** ⌥F12 로 터미널을 열고 셸이 떴다고 알린다. 돌려주는 것은 그 스트림 id. */
async function openTerminal(): Promise<number> {
  await keydown(document.body, "F12", { altKey: true });
  await vi.waitFor(() => expect(host.creates).toHaveLength(1));
  const { streamId } = host.creates[0]!;
  await act(async () =>
    chunk(streamId, {
      kind: "started",
      terminalId: 1,
      pid: 10,
      shell: ["/bin/zsh", "-l"],
      cwd: "/wt/project",
    }),
  );
  await settle();
  return streamId;
}

it("opens a tab on ⌥F12 and starts the shell in the active worktree", async () => {
  await mount();
  expect(tabs()).toHaveLength(0);
  const streamId = await openTerminal();
  expect(host.creates[0]!.params).toMatchObject({ projectId: "p1", worktree: "/wt" });
  expect(tabs().map((t) => t.textContent)).toEqual(["로컬"]);
  await act(async () => chunk(streamId, { kind: "output", data: b64("hello 가나\r\n") }));
  await settle();
  const instance = windowTerminals().active()!;
  expect(instance.term.buffer.active.getLine(0)!.translateToString(true)).toBe("hello 가나");
});

it("sends Ctrl+C to the shell but takes ⌘W to close the terminal tab, not the window", async () => {
  await mount();
  const streamId = await openTerminal();
  textarea().focus();
  await keydown(textarea(), "KeyC", { key: "c", keyCode: 67, ctrlKey: true });
  await vi.waitFor(() => expect(host.inputs).toEqual(["\x03"]));
  const closeWindow = vi.spyOn(native, "closeWindow");
  await keydown(textarea(), "KeyW", { key: "w", keyCode: 87, metaKey: true });
  expect(tabs()).toHaveLength(0);
  expect(host.cancelled).toEqual([streamId]);
  expect(closeWindow).not.toHaveBeenCalled();
});

it("hides the terminal on ⌥F12 from inside it instead of sending the key to the shell", async () => {
  await mount();
  await openTerminal();
  textarea().focus();
  const collapsed = () => document.querySelectorAll(".dv-edge-collapsed").length;
  const before = collapsed();
  // xterm 은 ⌥F12 를 셸의 이스케이프 시퀀스로 바꿔 보내고 이벤트를 멈춘다 — 양보하지 않으면 창이 이 키를 받지 못한다.
  await keydown(textarea(), "F12", { key: "F12", keyCode: 123, altKey: true });
  expect(host.inputs).toEqual([]);
  expect(windowTerminals().store.getState().tabs).toHaveLength(1);
  // 툴 윈도우 토글이 돌았다: 하단 dock 이 접힌다.
  expect(collapsed()).toBe(before + 1);
});

it("drops the menu's Close that the same ⌘W press brings after closing the terminal tab", async () => {
  await mount();
  await openTerminal();
  textarea().focus();
  const closeWindow = vi.spyOn(native, "closeWindow");
  await act(async () => {
    textarea().dispatchEvent(
      new KeyboardEvent("keydown", {
        code: "KeyW",
        key: "w",
        keyCode: 87,
        metaKey: true,
        bubbles: true,
        cancelable: true,
      }),
    );
    // 같은 눌림이 네이티브 메뉴의 「닫기」(window.close) accelerator 로도 왔다.
    menuAction!("window.close");
  });
  await settle();
  expect(tabs()).toHaveLength(0);
  expect(closeWindow).not.toHaveBeenCalled();
});

it("sends a Korean composition to the shell once, as composed", async () => {
  await mount();
  await openTerminal();
  const ta = textarea();
  ta.focus();
  // WebKit 이 두벌식으로 「가나다」 를 칠 때처럼: 키마다 keyCode 229 의 keydown, 조합 이벤트, 조합 중 input.
  const steps = ["ㄱ", "가", "간", "가나", "가낟", "가나다"];
  await act(async () => ta.dispatchEvent(new CompositionEvent("compositionstart", { data: "" })));
  for (const text of steps) {
    await act(async () => {
      ta.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Process", code: "KeyR", keyCode: 229, bubbles: true }),
      );
      ta.value = text;
      ta.dispatchEvent(new CompositionEvent("compositionupdate", { data: text }));
      ta.dispatchEvent(
        new InputEvent("input", { data: text, inputType: "insertCompositionText", bubbles: true }),
      );
    });
  }
  await act(async () =>
    ta.dispatchEvent(new CompositionEvent("compositionend", { data: "가나다" })),
  );
  await settle();
  expect(host.inputs.join("")).toBe("가나다");
  expect(host.inputs).toEqual(["가나다"]);
});

it("repaints open terminals when the appearance changes", async () => {
  await mount();
  await openTerminal();
  const theme = parseTheme(defaultTheme);
  const term = windowTerminals().active()!.term;
  expect(term.options.theme?.background).toBe(theme.variants.light!["terminal.background"]);
  expect(term.options.theme?.blue).toBe(theme.variants.light!["terminal.ansi.blue"]);
  await act(async () => store.getState().setPreference("dark"));
  await settle();
  expect(term.options.theme?.background).toBe(theme.variants.dark!["terminal.background"]);
  expect(term.options.theme?.blue).toBe(theme.variants.dark!["terminal.ansi.blue"]);
});

it("marks the tab when the shell exits", async () => {
  await mount();
  const streamId = await openTerminal();
  await act(async () => {
    chunk(streamId, { kind: "exited", code: 0 });
    host.send({ method: "stream.end", params: { streamId, status: "completed" } });
  });
  await settle();
  expect(tabs()[0]!.dataset.ended).toBe("true");
  expect(tabs()[0]!.textContent).toContain("끝남");
});

it("pastes the quoted paths of files dropped on the terminal", async () => {
  await mount();
  await openTerminal();
  await vi.waitFor(() => expect(host.drop).toBeDefined());
  const view = document.querySelector<HTMLElement>("[data-testid=terminal-view]")!.parentElement!;
  vi.spyOn(view, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 400, 800, 200));
  // Retina: 드롭 위치는 물리 픽셀(CSS 의 2배)로 온다. (20, 900) → CSS (10, 450) 은 터미널 안.
  Object.defineProperty(window, "devicePixelRatio", { configurable: true, value: 2 });
  await act(async () =>
    host.drop!({
      payload: { type: "drop", paths: ["/a/b.txt", "/a/my file.txt"], position: { x: 20, y: 900 } },
    }),
  );
  await settle();
  // 위치(물리 픽셀 / devicePixelRatio)가 터미널 밖이면 붙이지 않는다.
  await act(async () =>
    host.drop!({ payload: { type: "drop", paths: ["/x"], position: { x: 20, y: 10 } } }),
  );
  await settle();
  expect(host.inputs.join("")).toBe("/a/b.txt '/a/my file.txt' ");
});
