// @vitest-environment happy-dom
// 프로젝트 창(App kind="project", `?project=p1`)을 가짜 Host 에 붙여 신뢰 흐름을 본다: 처음 여는 프로젝트에서 다이얼로그가 한 번
// 뜨고, 「신뢰」 를 누르면 상태바 표시가 사라지며 다시 열어도 묻지 않는다. 다른 창이 먼저 물었으면 묻지 않고 상태바에서 연다.

import type { ConfigView, HostInfo, KeymapFile, Project, ProjectTrust } from "@pitwall/rpc-client";
import { act, StrictMode } from "react";
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
  /** Host 레지스트리의 신뢰 행. 없으면 아직 묻지 않음. */
  trust: undefined as boolean | undefined,
  claims: 0,
  sets: [] as boolean[],
  /** 있으면 `project.setTrust` 가 이 오류로 거절한다. */
  setError: undefined as { code: number; message: string; data?: string } | undefined,
  emit: (_topic: string, _payload: unknown) => {},
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
      const state = (): ProjectTrust => ({
        projectId: "p1",
        ownerId: "p1",
        state: host.trust === undefined ? "unknown" : host.trust ? "trusted" : "untrusted",
      });
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
          case "task.list":
            return reply([]);
          case "project.get":
            return reply({
              id: "p1",
              name: "demo",
              host: "local",
              roots: ["/work/demo"],
              favorite: false,
              tags: [],
              configMode: "repository",
              slug: "demo",
              createdAt: 1,
              openedAt: null,
            } satisfies Project);
          case "project.trust":
            return reply(state());
          case "project.claimTrustPrompt": {
            host.claims += 1;
            const first = host.trust === undefined;
            if (first) host.trust = false;
            return reply(first);
          }
          case "project.setTrust": {
            if (host.setError) return send({ id: m.id, error: host.setError });
            const { trusted } = m.params as { trusted: boolean };
            host.sets.push(trusted);
            host.trust = trusted;
            reply(state());
            return host.emit("project.changed", { kind: "trust", projectId: "p1" });
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
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.trust = undefined;
  host.claims = 0;
  host.sets = [];
  host.setError = undefined;
  localStorage.clear();
});

afterEach(() => {
  unmount();
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 20)));

function unmount() {
  act(() => root?.unmount());
  root = undefined;
}

/** 앱처럼 StrictMode 로 띄운다(src/main.tsx). */
async function mount() {
  const element = document.createElement("div");
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
      <StrictMode>
        <ThemeStoreProvider store={store}>
          <App kind="project" native={native} />
        </ThemeStoreProvider>
      </StrictMode>,
    ),
  );
  await settle();
}

const dialog = () => document.querySelector<HTMLElement>("[data-testid=trust-dialog]");
const trustStatus = () => document.querySelector<HTMLElement>("[data-testid=status-trust]");

async function click(element: HTMLElement | null | undefined) {
  if (!element) throw new Error("누를 요소가 없다");
  await act(async () => element.click());
  await settle();
}

function button(label: string): HTMLButtonElement | undefined {
  return [...(dialog()?.querySelectorAll("button") ?? [])].find((b) => b.textContent === label);
}

async function findAndRun(label: string) {
  await act(async () => {
    document.body.dispatchEvent(
      new KeyboardEvent("keydown", {
        code: "KeyA",
        key: "KeyA",
        metaKey: true,
        shiftKey: true,
        bubbles: true,
        cancelable: true,
      }),
    );
  });
  await settle();
  const input = document.querySelector<HTMLInputElement>("[cmdk-input]")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, label);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();
  const item = [...document.querySelectorAll<HTMLElement>("[cmdk-item]")].find((e) =>
    e.textContent?.startsWith(label),
  );
  await click(item);
}

it("asks once for a new project, and trusting it clears the status item for good", async () => {
  await mount();
  expect(host.claims).toBe(1);
  expect(dialog()?.textContent).toContain("「demo」 프로젝트를 신뢰할까?");
  expect(trustStatus()?.textContent).toBe("런타임 환경 미적용(신뢰 필요)");

  await click(button("신뢰"));
  expect(host.sets).toEqual([true]);
  expect(dialog()).toBeNull();
  expect(trustStatus()).toBeNull();

  // 다시 열어도 묻지 않는다.
  unmount();
  await mount();
  expect(host.claims).toBe(2);
  expect(dialog()).toBeNull();
  expect(trustStatus()).toBeNull();
});

it("does not ask when another window already did, and opens the dialog from the status bar", async () => {
  host.trust = false;
  await mount();
  expect(host.claims).toBe(1);
  expect(dialog()).toBeNull();

  await click(trustStatus());
  expect(dialog()).not.toBeNull();
  await click(button("신뢰하지 않음"));
  expect(host.sets).toEqual([false]);
  expect(dialog()).toBeNull();
  expect(trustStatus()).not.toBeNull();
});

it("untrusts from an action and reports a failed save", async () => {
  host.trust = true;
  await mount();
  expect(trustStatus()).toBeNull();
  await findAndRun("이 프로젝트 신뢰 해제");
  expect(host.sets).toEqual([false]);
  expect(trustStatus()).not.toBeNull();

  host.setError = { code: -32003, message: "storage unavailable", data: "the registry is locked" };
  await click(trustStatus());
  await click(button("신뢰"));
  expect(document.body.textContent).toContain("프로젝트 신뢰를 저장하지 못했다");
  expect(document.body.textContent).toContain("the registry is locked");
  expect(trustStatus()).not.toBeNull();
});
