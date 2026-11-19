// @vitest-environment happy-dom
// 프로젝트 창(App kind="project")을 가짜 Host 에 붙여 「실행」 툴 윈도우를 띄운다(17-m6-plan M6-06): 다시 불러온 창이 `run.list` ·
// `run.attach { since: 0 }` 으로 앞 출력을 보인다(완료 기준 ③), 에이전트가 띄운 실행의 탭이 생기고 상태바에 수가 보인다(④ · ⑦),
// 실행이 남은 채 창을 닫으면 확인이 뜨고 처음 포커스가 [취소]다(⑤). StrictMode 로 그린다(test-support/strict-mode.ts).

import type { ConfigView, HostInfo, KeymapFile, RunInfo } from "@pitwall/rpc-client";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import type { KeymapNative } from "../keymap/native";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { runInfo } from "./fake-run-host";
import { windowRuns } from "./run-tabs";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const host = vi.hoisted(() => ({
  runs: [] as unknown[],
  attaches: [] as { streamId: number; params: Record<string, unknown> }[],
  runSubscriptions: [] as number[],
  /** 구독마다 들은 토픽 — 가짜 Host 는 들은 토픽만 보낸다. */
  topics: new Map<number, string[]>(),
  calls: [] as string[],
  send: (_m: object) => {},
}));

const opened = vi.hoisted(() => [] as string[]);
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: async (url: string) => {
    opened.push(url);
  },
}));

vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: async () => () => {} }),
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
        host.calls.push(m.method);
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
          case "events.subscribe": {
            const id = next++;
            const topics = (m.params as { topics?: string[] }).topics ?? [];
            host.topics.set(id, topics);
            if (topics.includes("run.started")) host.runSubscriptions.push(id);
            return reply({ subscriptionId: id });
          }
          case "run.list":
            return reply({ runs: host.runs });
          case "run.attach": {
            const streamId = next++;
            host.attaches.push({ streamId, params: m.params });
            return reply({ streamId });
          }
          case "stream.cancel":
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
let guard: (() => Promise<boolean>) | undefined;
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
  guardClose: (shouldClose) => {
    guard = shouldClose;
    return () => {
      if (guard === shouldClose) guard = undefined;
    };
  },
};

let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  vi.spyOn(console, "warn").mockImplementation(() => {});
  host.runs = [];
  host.attaches = [];
  host.runSubscriptions = [];
  host.calls = [];
  localStorage.clear();
  sessionStorage.clear();
  window.history.replaceState(null, "", "/?window=project&project=p1");
});

afterEach(() => {
  act(() => windowRuns().disposeAll());
  act(() => root?.unmount());
  root = undefined;
  guard = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 30)));

async function mount() {
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
        <App kind="project" native={native} />
      </ThemeStoreProvider>,
    ),
  );
  await settle();
}

function chunk(streamId: number, data: object) {
  host.send({ method: "stream.chunk", params: { streamId, data } });
}

function event(topic: string, run: RunInfo) {
  for (const subscriptionId of host.runSubscriptions.filter((id) =>
    host.topics.get(id)?.includes(topic),
  ))
    host.send({
      method: "events.event",
      params: { subscriptionId, projectId: "p1", topic, payload: run },
    });
}

const b64 = (text: string) => btoa(String.fromCharCode(...new TextEncoder().encode(text)));
const tabEls = () => [...document.querySelectorAll<HTMLElement>("[data-testid=run-tab]")];

async function openRunToolWindow() {
  await act(async () => menuAction!("toolWindow.run"));
  await settle();
}

it("shows a reloaded window the output its runs printed before", async () => {
  // 창을 다시 불러온 때: Host 에 실행(끝남)과 그 출력이 남아 있다.
  const run = runInfo({ runId: "r1", state: "exited", exit: { code: 0, signal: null } });
  host.runs = [run];
  await mount();
  await openRunToolWindow();
  await vi.waitFor(() => expect(host.attaches).toHaveLength(1));
  const { streamId, params } = host.attaches[0]!;
  expect(params).toEqual({ runId: "r1", since: 0 });
  await act(async () => {
    chunk(streamId, { kind: "attached", run, from: 0, dropped: 0 });
    chunk(streamId, { kind: "output", offset: 0, data: b64("VITE ready in 300 ms\r\n") });
    chunk(streamId, { kind: "exited", exit: { code: 0, signal: null } });
  });
  await settle();
  expect(tabEls().map((t) => t.dataset.runId)).toEqual(["r1"]);
  expect(tabEls()[0]!.querySelector("[data-testid=run-tab-exit]")?.textContent).toBe("끝 0");
  const term = windowRuns().console("r1")!.term;
  expect(term.buffer.active.getLine(0)!.translateToString(true)).toBe("VITE ready in 300 ms");
});

it("opens a tab for a run an agent started and counts it in the status bar", async () => {
  await mount();
  await openRunToolWindow();
  await vi.waitFor(() => expect(host.runSubscriptions.length).toBeGreaterThan(0));
  await act(async () =>
    event(
      "run.started",
      runInfo({
        runId: "a1",
        name: "test",
        launchedBy: { kind: "agent", session: "s1", client: "claude-code" },
      }),
    ),
  );
  await settle();
  expect(tabEls().map((t) => [t.dataset.runId, t.dataset.launchedBy])).toEqual([["a1", "agent"]]);
  expect(document.querySelector("[data-testid=status-run]")?.textContent).toBe("실행 1");
  // 끝나면 상태바 수가 사라진다.
  await act(async () =>
    event(
      "run.exited",
      runInfo({
        runId: "a1",
        state: "exited",
        exit: { code: 1, signal: null },
        launchedBy: { kind: "agent", session: "s1", client: "claude-code" },
      }),
    ),
  );
  await settle();
  expect(document.querySelector("[data-testid=status-run]")).toBeNull();
});

it("asks before closing the window with runs left, Cancel focused first", async () => {
  host.runs = [
    runInfo({ runId: "r1", name: "dev" }),
    runInfo({
      runId: "a1",
      name: "e2e",
      launchedBy: { kind: "agent", session: "s1", client: "claude-code" },
    }),
    runInfo({ runId: "old", state: "exited", exit: { code: 0, signal: null } }),
  ];
  await mount();
  expect(guard).toBeDefined();
  let answer: Promise<boolean> | undefined;
  await act(async () => {
    answer = guard!();
  });
  await settle();
  const dialog = document.querySelector("[data-testid=run-close-dialog]")!;
  expect(dialog.textContent).toContain("실행 중 2개");
  const items = [...dialog.querySelectorAll<HTMLElement>("[data-testid=run-close-item]")];
  expect(items.map((i) => [i.dataset.launchedBy, i.textContent])).toEqual([
    ["window", "dev사람이 띄움"],
    ["agent", "e2e에이전트가 띄움(claude-code)"],
  ]);
  expect(document.activeElement?.textContent).toBe("취소");
  await act(async () => (document.activeElement as HTMLButtonElement).click());
  await expect(answer).resolves.toBe(false);

  // Esc(바깥 누르기와 같은 닫기)도 [취소]다 — 실행을 끄는 쪽으로 가지 않는다.
  await act(async () => {
    answer = guard!();
  });
  await settle();
  await act(async () =>
    document.activeElement!.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", code: "Escape", bubbles: true }),
    ),
  );
  await expect(answer).resolves.toBe(false);
  expect(document.querySelector("[data-testid=run-close-dialog]")).toBeNull();

  // 「끝내고 닫기」 는 닫기를 막지 않는다(끝내기는 창이 사라질 때 Host 가 한다).
  await act(async () => {
    answer = guard!();
  });
  await settle();
  const confirm = [...document.querySelectorAll("button")].find(
    (b) => b.textContent === "끝내고 닫기",
  )!;
  await act(async () => confirm.click());
  await expect(answer).resolves.toBe(true);
});

it("closes without asking when nothing is running", async () => {
  host.runs = [runInfo({ runId: "old", state: "exited", exit: { code: 0, signal: null } })];
  await mount();
  let answer: Promise<boolean> | undefined;
  await act(async () => {
    answer = guard!();
  });
  await expect(answer).resolves.toBe(true);
  expect(document.querySelector("[data-testid=run-close-dialog]")).toBeNull();
});

it("shows the ports a run listens on and opens them in the browser", async () => {
  opened.length = 0;
  host.runs = [runInfo({ runId: "r1", name: "dev" })];
  await mount();
  await openRunToolWindow();
  await vi.waitFor(() => expect(host.runSubscriptions.length).toBeGreaterThan(0));
  expect(document.querySelector("[data-testid=run-tab-port]")).toBeNull();
  await act(async () =>
    event(
      "run.ports",
      runInfo({
        runId: "r1",
        name: "dev",
        ports: [
          { port: 5173, allAddresses: false, addresses: ["127.0.0.1"] },
          { port: 8080, allAddresses: true, addresses: ["0.0.0.0", "::"] },
          { port: 9229, allAddresses: false, addresses: ["::1"] },
        ],
      }),
    ),
  );
  await settle();
  const chips = [...document.querySelectorAll<HTMLElement>("[data-testid=run-tab-port]")];
  expect(chips.map((c) => [c.dataset.port, c.dataset.allAddresses])).toEqual([
    ["5173", "false"],
    ["8080", "true"],
    ["9229", "false"],
  ]);
  expect(chips[0]!.textContent).toBe(":5173");
  // 「모든 주소」 표지는 0.0.0.0 · :: 에 듣는 포트에만.
  expect(chips[1]!.querySelector("[data-testid=run-port-all-addresses]")?.textContent).toBe(
    "모든 주소",
  );
  expect(chips[0]!.querySelector("[data-testid=run-port-all-addresses]")).toBeNull();
  expect(chips[0]!.title).toBe("http://localhost:5173 열기");
  // 상태바에도 보인다.
  const status = [...document.querySelectorAll<HTMLElement>("[data-testid=status-run-port]")];
  expect(status.map((c) => c.dataset.port)).toEqual(["5173", "8080", "9229"]);

  // 누르면 바깥 브라우저 — 모든 주소여도 localhost, IPv6 에만 들으면 [::1].
  await act(async () => chips[0]!.click());
  await act(async () => chips[1]!.click());
  await act(async () => chips[2]!.click());
  await act(async () => status[0]!.click());
  expect(opened).toEqual([
    "http://localhost:5173",
    "http://localhost:8080",
    "http://[::1]:9229",
    "http://localhost:5173",
  ]);

  // 끝나면 칩이 사라진다(끝난 실행에 포트가 실려 와도 그리지 않는다).
  await act(async () =>
    event(
      "run.exited",
      runInfo({
        runId: "r1",
        state: "exited",
        exit: { code: 0, signal: null },
        ports: [{ port: 5173, allAddresses: false, addresses: ["127.0.0.1"] }],
      }),
    ),
  );
  await settle();
  expect(document.querySelector("[data-testid=run-tab-port]")).toBeNull();
  expect(document.querySelector("[data-testid=status-run-port]")).toBeNull();
});
