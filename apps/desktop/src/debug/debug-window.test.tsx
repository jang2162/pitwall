// @vitest-environment happy-dom
// 프로젝트 창(App kind="project")을 가짜 Host 에 붙여 디버그 화면을 띄운다(17-m6-plan M6-18): 디버그 세션이 멈추면 「디버그」 툴
// 윈도우가 열리고 맨 위 프레임으로 간다 — 루트 밖 프레임은 `debug.source` 토큰으로 읽기 전용 탭에 열리고 파일 RPC 로는 읽지 않는다
// (완료 기준 ⑤). 창이 앞에 없을 때의 주의 요청을 부르고, 세션이 끝나면 화면이 「끝남」 으로 정리된다. StrictMode 로 그린다.

import type { ConfigView, HostInfo, KeymapFile, RunInfo } from "@pitwall/rpc-client";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import type { KeymapNative } from "../keymap/native";
import { runInfo } from "../run/fake-run-host";
import { windowRuns } from "../run/run-tabs";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { windowDebug } from "./debug-session";

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
  frames: [] as unknown[],
  sources: [] as unknown[],
  fsPaths: [] as string[],
  send: (_m: object) => {},
}));

const invoked = vi.hoisted(() => [] as string[]);
vi.mock("@tauri-apps/api/core", async (original) => ({
  ...(await original<typeof import("@tauri-apps/api/core")>()),
  invoke: async (command: string) => {
    invoked.push(command);
  },
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
            if (topics.includes("debug.stopped")) host.runSubscriptions.push(id);
            return reply({ subscriptionId: id });
          }
          case "run.list":
            return reply({ runs: host.runs });
          case "debug.breakpoints.list":
            return reply({ files: [], problem: null });
          case "debug.threads":
            return reply({ threads: [{ id: 1, name: "main" }] });
          case "debug.stack":
            return reply({ frames: host.frames, totalFrames: null });
          case "debug.scopes":
            return reply({ scopes: [] });
          case "debug.source":
            host.sources.push(m.params);
            return reply({
              text: "pub fn lib() {}\n",
              path: "/rustlib/lib.rs",
              adapter: "codelldb",
            });
          case "fs.stat":
          case "fs.read":
            host.fsPaths.push((m.params as { path: string }).path);
            return send({ id: m.id, error: { code: -32004, message: "not found" } });
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

let guard: (() => Promise<boolean>) | undefined;
const native: KeymapNative = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
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
  host.frames = [];
  host.sources = [];
  host.fsPaths = [];
  invoked.length = 0;
  localStorage.clear();
  sessionStorage.clear();
  window.history.replaceState(null, "", "/?window=project&project=p1");
});

afterEach(() => {
  act(() => windowRuns().disposeAll());
  // 창별 객체를 시험마다 새로(모듈 변수) — 끝난 세션이 다음 시험에 남지 않게.
  windowDebug().store.setState({ sessions: [], activeRunId: undefined });
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

function event(topic: string, payload: unknown) {
  for (const subscriptionId of host.runSubscriptions.filter((id) =>
    host.topics.get(id)?.includes(topic),
  ))
    host.send({
      method: "events.event",
      params: { subscriptionId, projectId: "p1", topic, payload },
    });
}

const debugRun = (patch: Partial<RunInfo> = {}) =>
  runInfo({ runId: "d1", kind: "debug", name: "app", config: "app", ...patch });

function stopped() {
  return {
    projectId: "p1",
    runId: "d1",
    session: 1,
    reason: "breakpoint",
    description: null,
    threadId: 1,
    text: null,
    allThreadsStopped: true,
    hitBreakpointIds: [],
  };
}

it("opens a frame outside the project read-only through its token and ends cleanly", async () => {
  host.runs = [debugRun()];
  host.frames = [
    {
      id: 11,
      name: "lib",
      path: "/rustlib/lib.rs",
      sourceReference: null,
      line: 1,
      column: 1,
      presentationHint: null,
      token: "tok-1",
    },
    {
      id: 12,
      name: "main",
      path: "/main/src/main.rs",
      sourceReference: null,
      line: 4,
      column: 1,
      presentationHint: null,
      token: null,
    },
  ];
  await mount();
  await vi.waitFor(() => expect(host.runSubscriptions.length).toBeGreaterThan(0));
  await settle();
  await act(async () => event("debug.stopped", stopped()));
  await settle();
  await settle();

  // 툴 윈도우: 멈춤 · 프레임 둘.
  await vi.waitFor(() =>
    expect(document.querySelector("[data-debug-state=stopped]")).not.toBeNull(),
  );
  expect(document.querySelectorAll("[data-frame]")).toHaveLength(2);
  // 루트 밖 프레임은 토큰으로 읽기 전용 탭에 — 파일 RPC 로 그 경로를 읽지 않는다.
  await vi.waitFor(() =>
    expect(document.querySelector("[data-outside-source=tok-1]")).not.toBeNull(),
  );
  expect(host.sources).toEqual([{ runId: "d1", token: "tok-1" }]);
  expect(host.fsPaths).not.toContain("/rustlib/lib.rs");
  expect(document.querySelector("[data-outside-source-bar]")?.textContent).toContain(
    "디버그 소스 · 읽기 전용 · codelldb",
  );
  const text = document.querySelector<HTMLElement>("[data-outside-source-text] .cm-content");
  expect(text?.textContent).toContain("pub fn lib()");
  // 창이 앞에 없을 때의 주의 요청(네이티브가 앞에 있는지 가른다).
  expect(invoked).toContain("debug_stopped_attention");

  // 루트 안 프레임은 파일로 연다.
  await act(async () => document.querySelector<HTMLElement>("[data-frame='12']")!.click());
  await settle();
  expect(host.fsPaths).toContain("/main/src/main.rs");

  // 끝 — 툴바 · 프레임을 걷고 「끝남」.
  await act(async () => event("debug.ended", { projectId: "p1", runId: "d1" }));
  await settle();
  expect(document.querySelector("[data-debug-state=ended]")?.textContent).toBe("끝남");
  expect(document.querySelectorAll("[data-frame]")).toHaveLength(0);
  expect(windowDebug().current.getState().frame).toBeUndefined();
});

it("ends the session when its run exits without a debug.ended (the adapter died)", async () => {
  host.runs = [debugRun()];
  host.frames = [
    {
      id: 12,
      name: "main",
      path: "/main/src/main.rs",
      sourceReference: null,
      line: 4,
      column: 1,
      presentationHint: null,
      token: null,
    },
  ];
  await mount();
  await vi.waitFor(() => expect(host.runSubscriptions.length).toBeGreaterThan(0));
  await settle();
  await act(async () => event("debug.stopped", stopped()));
  await settle();
  await vi.waitFor(() =>
    expect(document.querySelector("[data-debug-state=stopped]")).not.toBeNull(),
  );
  expect(windowDebug().current.getState().frame).toEqual({ path: "/main/src/main.rs", line: 4 });
  await act(async () =>
    event("run.exited", debugRun({ state: "exited", exit: { code: null, signal: "SIGKILL" } })),
  );
  await settle();
  expect(document.querySelector("[data-debug-state=ended]")).not.toBeNull();
  expect(windowDebug().current.getState().frame).toBeUndefined();
  expect(windowDebug().debugging()).toBe(false);
});
