// @vitest-environment happy-dom
// 프로젝트 창(App kind="project")을 가짜 Host 에 붙여 띄운다: UI 언어를 바꾸면 툴 윈도우 이름 · 툴 윈도우 액션 · 상태바가 재시작
// 없이 바뀌는 것, 백그라운드 작업이 상태바에 진행률로 보이고 취소되는 것, 오류 알림이 토스트로 뜨고 알림 툴 윈도우에 남는 것.

import type { ConfigView, HostInfo, KeymapFile, TaskInfo } from "@pitwall/rpc-client";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import type { KeymapNative } from "../keymap/native";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const host = vi.hoisted(() => ({
  /** `i18n.get` 이 고른 언어. 바꾼 뒤 `config.changed` 를 내면 창이 다시 읽는다. */
  locale: "ko",
  /** `task.list` 의 응답. */
  tasks: [] as unknown[],
  cancelled: [] as number[],
  demoParams: [] as unknown[],
  emit: (_topic: string, _payload: unknown) => {},
  /** 열린 데모 스트림을 끝낸다. */
  endDemo: () => {},
}));

vi.mock("../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../keymap/default-keymap");
  const { i18nState } = await import("../i18n/testing");
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
          case "i18n.get":
            return reply(i18nState(host.locale));
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
          case "task.list":
            return reply(host.tasks);
          case "task.cancel": {
            const { taskId } = m.params as { taskId: number };
            host.cancelled.push(taskId);
            reply(true);
            host.emit("task.finished", { taskId, title: "demo", status: "cancelled" });
            return host.endDemo();
          }
          case "host.demoTask": {
            const params = m.params as { steps: number };
            host.demoParams.push(params);
            if (params.steps === 0) {
              return send({
                id: m.id,
                error: { code: -32602, message: "steps must be 1..=1000" },
              });
            }
            const streamId = next++;
            host.endDemo = () =>
              send({ method: "stream.end", params: { streamId, status: "completed" } });
            return reply({ streamId });
          }
          case "events.subscribe": {
            const id = next++;
            subs.set(id, m.params as { projectId?: string; topics: string[] });
            return reply({ subscriptionId: id });
          }
          case "events.unsubscribe":
            return reply(null);
          default:
            return send({ id: m.id, error: { code: -32601, message: `no ${m.method}` } });
        }
      });
      host.emit = (topic, payload) => {
        for (const [subscriptionId, s] of subs) {
          if (s.projectId !== undefined || !s.topics.includes(topic)) continue;
          send({
            method: "events.event",
            params: { subscriptionId, projectId: null, topic, payload },
          });
        }
      };
      return { transport: client, close: async () => {} };
    },
  };
});

const native: KeymapNative = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
};

let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  host.locale = "ko";
  host.tasks = [];
  host.cancelled = [];
  host.demoParams = [];
  localStorage.clear();
});

afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 20)));

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

async function press(code: string, mods: Partial<KeyboardEventInit> = {}) {
  await act(async () => {
    (document.activeElement ?? document.body).dispatchEvent(
      new KeyboardEvent("keydown", { code, key: code, bubbles: true, cancelable: true, ...mods }),
    );
    document.body.dispatchEvent(new KeyboardEvent("keyup", { code, key: code, bubbles: true }));
  });
  await settle();
}

/** 액션 찾기(⌘⇧A)에 `query` 를 넣었을 때의 항목 글자. */
async function findActions(query: string): Promise<HTMLElement[]> {
  await press("KeyA", { metaKey: true, shiftKey: true });
  const input = document.querySelector<HTMLInputElement>("[cmdk-input]")!;
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    setter.call(input, query);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();
  return [...document.querySelectorAll<HTMLElement>("[cmdk-item]")];
}

async function runAction(label: string) {
  const item = (await findActions(label)).find((e) => e.textContent?.startsWith(label));
  if (!item) throw new Error(`액션 ${label} 이 없다`);
  await act(async () => item.click());
  await settle();
}

const tabTexts = () => [...document.querySelectorAll(".dv-tab")].map((t) => t.textContent);
const statusBar = () => document.querySelector("footer[aria-label]")!;
const statusItem = (id: string) =>
  document.querySelector<HTMLElement>(`[data-status-item="${id}"]`);
const leftBarLabels = () =>
  [...document.querySelectorAll("nav button")].map((b) => b.getAttribute("aria-label"));

it("switches tool window names, their actions and the status bar between ko and en without a restart", async () => {
  await mount();
  expect(tabTexts()).toContain("터미널");
  expect(leftBarLabels()).toContain("프로젝트");
  expect(statusBar().getAttribute("aria-label")).toBe("상태바");
  expect(statusItem("appearance")!.textContent).toBe("라이트");
  expect(statusItem("notifications")!.querySelector("button")!.getAttribute("aria-label")).toBe(
    "알림",
  );
  expect((await findActions("터미널")).map((e) => e.textContent)).toContain("터미널 툴 윈도우⌥F12");
  await press("Escape");

  host.locale = "en";
  await act(async () => host.emit("config.changed", {}));
  await settle();

  expect(tabTexts()).toContain("Terminal");
  expect(tabTexts()).not.toContain("터미널");
  expect(leftBarLabels()).toContain("Project");
  expect(statusBar().getAttribute("aria-label")).toBe("Status bar");
  expect(statusItem("appearance")!.textContent).toBe("Light");
  expect(statusItem("notifications")!.querySelector("button")!.getAttribute("aria-label")).toBe(
    "Notifications",
  );
  expect((await findActions("Terminal")).map((e) => e.textContent)).toContain(
    "Terminal Tool Window⌥F12",
  );
});

it("shows a demo task's progress in the status bar and stops it with the cancel button", async () => {
  await mount();
  expect(statusItem("tasks")!.textContent).toBe("");

  await runAction("데모 작업 실행");
  expect(host.demoParams).toEqual([{ steps: 50, intervalMs: 200 }]);
  await act(async () =>
    host.emit("task.progress", {
      taskId: 7,
      title: "demo",
      progress: { done: 5, total: 50, message: null },
    }),
  );
  await settle();
  const item = statusItem("tasks")!;
  expect(item.textContent).toContain("demo");
  expect(item.textContent).toContain("10%");
  expect(item.querySelector("[role=progressbar]")!.getAttribute("aria-valuenow")).toBe("10");

  const cancel = item.querySelector<HTMLButtonElement>('button[aria-label="「demo」 취소"]')!;
  await act(async () => cancel.click());
  await settle();
  expect(host.cancelled).toEqual([7]);
  expect(statusItem("tasks")!.textContent).toBe("");
  // 취소는 실패가 아니다 — 알림을 내지 않는다.
  expect(document.querySelector("[data-sonner-toast]")).toBeNull();
});

it("shows tasks that were already running when the window opened", async () => {
  host.tasks = [
    {
      taskId: 3,
      title: "index",
      projectId: null,
      progress: { done: 0, total: null, message: null },
    },
  ] satisfies TaskInfo[];
  await mount();
  const item = statusItem("tasks")!;
  expect(item.textContent).toContain("index");
  // 전체 양을 모르면 불확정 막대다.
  expect(item.querySelector("[role=progressbar]")!.hasAttribute("aria-valuenow")).toBe(false);
});

it("toasts an error, keeps it in the notifications tool window and highlights the status bar until seen", async () => {
  await mount();
  const bell = () => statusItem("notifications")!.querySelector("button")!;
  expect(bell().dataset.unseenErrors).toBe("0");
  // 기본 레이아웃은 알림 툴 윈도우가 보인다 — 보이는 동안 온 오류는 본 것이다. 접어 두고 시작한다.
  await runAction("모든 툴 윈도우 숨기기");

  await runAction("데모 작업 실행(잘못된 인자");
  await act(async () =>
    host.emit("task.finished", { taskId: 9, title: "index", status: "failed", error: "disk gone" }),
  );
  await settle();

  const toasts = [...document.querySelectorAll("[data-sonner-toast]")].map((t) => t.textContent);
  expect(toasts.some((t) => t?.includes("작업을 시작하지 못했다"))).toBe(true);
  expect(toasts.some((t) => t?.includes("작업 실패: index") && t.includes("disk gone"))).toBe(true);
  expect(bell().dataset.unseenErrors).toBe("2");
  expect(bell().getAttribute("aria-label")).toBe("알림 — 보지 않은 오류 2개");

  await act(async () => bell().click());
  await settle();
  const tool = document.querySelector('[data-tool-window="notifications"]')!;
  const entries = [...tool.querySelectorAll("li")].map((li) => li.textContent);
  expect(entries[0]).toContain("작업 실패: index");
  expect(entries[0]).toContain("disk gone");
  expect(entries[1]).toContain("steps must be 1..=1000");
  expect(bell().dataset.unseenErrors).toBe("0");
});

it("counts an error as seen when it arrives while the notifications tool window is in view", async () => {
  await mount();
  await act(async () =>
    host.emit("task.finished", { taskId: 9, title: "index", status: "failed", error: "disk gone" }),
  );
  await settle();
  expect(statusItem("notifications")!.querySelector("button")!.dataset.unseenErrors).toBe("0");
  expect(document.querySelector('[data-tool-window="notifications"] li')!.textContent).toContain(
    "disk gone",
  );
});
