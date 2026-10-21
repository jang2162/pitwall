// @vitest-environment happy-dom
// 상태바의 런타임 항목(M1-16): 프로젝트 창(App kind="project", `?project=p1`)을 가짜 Host 에 붙여 `runtime.get` 의 상태별 표시를
// 본다 — 물러났으면 경고(mise 미신뢰 등)와 원문 이유, mise 가 만들었으면 「mise」, 로그인 셸만이면 없음. Host 가 그 프로젝트에
// `runtime.changed` 를 내면 다시 읽고, 누르면 「환경 다시 읽기」(`runtime.reload`)다.

import type {
  ConfigView,
  HostInfo,
  KeymapFile,
  Project,
  ProjectTrust,
  RuntimeInfo,
} from "@pitwall/rpc-client";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import { bundledTranslator } from "../i18n/testing";
import type { KeymapNative } from "../keymap/native";
import { projectStrings } from "../layout/strings";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { describeRuntime } from "./runtime-status";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const host = vi.hoisted(() => ({
  /** `runtime.get` 의 응답. */
  runtime: undefined as unknown,
  gets: 0,
  reloads: [] as unknown[],
  /** 프로젝트 범위로 구독한 창에 이벤트를 보낸다. 연결마다 바뀐다. */
  emit: (_projectId: string, _topic: string, _payload: unknown) => {},
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
            return reply({
              projectId: "p1",
              ownerId: "p1",
              state: "trusted",
            } satisfies ProjectTrust);
          case "project.claimTrustPrompt":
            return reply(false);
          case "project.storage":
            return reply({ kind: "ok", path: "/data/state.db" });
          case "runtime.get":
            host.gets += 1;
            return reply(host.runtime);
          case "runtime.reload":
            host.reloads.push(m.params);
            return reply(null);
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
      host.emit = (projectId, topic, payload) => {
        for (const [subscriptionId, s] of subs) {
          if (s.projectId !== projectId || !s.topics.includes(topic)) continue;
          send({
            method: "events.event",
            params: { subscriptionId, projectId, topic, payload },
          });
        }
      };
      return { transport: client, close: async () => {} };
    },
  };
});

const T = projectStrings(bundledTranslator("ko")).runtime;

const SHELL: RuntimeInfo = {
  provider: "shell",
  source: { kind: "loginShell", shell: "/bin/zsh" },
  status: { kind: "ok" },
  tools: [],
};
const NOT_TRUSTED: RuntimeInfo = {
  provider: "mise",
  source: { kind: "loginShell", shell: "/bin/zsh" },
  status: {
    kind: "fallback",
    reason:
      "mise does not trust the config here: Config files in /work/demo/mise.toml are not trusted.",
    problem: "notTrusted",
  },
  tools: [],
};
const MISE: RuntimeInfo = {
  provider: "mise",
  source: { kind: "configFiles", files: ["/work/demo/mise.toml"] },
  status: { kind: "ok" },
  tools: [],
};

const native: KeymapNative = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
  closeWindow: async () => {},
};

let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(console, "info").mockImplementation(() => {});
  vi.spyOn(console, "warn").mockImplementation(() => {});
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.runtime = SHELL;
  host.gets = 0;
  host.reloads = [];
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

const item = () => document.querySelector<HTMLElement>("[data-testid=status-runtime]");

it("warns when mise fell back, and follows runtime.changed to the mise environment", async () => {
  host.runtime = NOT_TRUSTED;
  await mount();
  expect(item()?.textContent).toBe(T.fallback("notTrusted"));
  expect(item()?.dataset.warning).toBe("true");
  expect(item()?.title).toContain("are not trusted");
  expect(item()?.title).toContain("mise trust");

  // 사람이 `mise trust` 를 하고 버전 파일을 고쳤다 — Host 가 다시 계산하고 그 프로젝트에 `runtime.changed` 를 낸다.
  host.runtime = MISE;
  const gets = host.gets;
  await act(async () => host.emit("p1", "runtime.changed", { reason: "files" }));
  await settle();
  expect(host.gets).toBeGreaterThan(gets);
  expect(item()?.textContent).toBe(T.mise);
  expect(item()?.dataset.warning).toBeUndefined();
  expect(item()?.title).toContain("/work/demo/mise.toml");

  // 다른 프로젝트의 이벤트는 이 창과 무관하다.
  const after = host.gets;
  await act(async () => host.emit("p2", "runtime.changed", { reason: "files" }));
  await settle();
  expect(host.gets).toBe(after);
});

it("shows nothing for the plain login shell environment", async () => {
  await mount();
  expect(host.gets).toBeGreaterThan(0);
  expect(item()).toBeNull();
});

it("reloads the runtime environment when clicked", async () => {
  host.runtime = NOT_TRUSTED;
  await mount();
  await act(async () => item()!.click());
  await settle();
  expect(host.reloads).toEqual([{ projectId: "p1" }]);
});

it("words every fallback problem", () => {
  for (const problem of ["loginShell", "notInstalled", "notTrusted", "failed"] as const) {
    const view = describeRuntime(
      { ...SHELL, status: { kind: "fallback", reason: "why", problem } },
      T,
    );
    expect(view?.warning).toBe(true);
    expect(view?.label).toBe(T.fallback(problem));
    expect(view?.hint).toContain("why");
  }
  // 문장은 갈래마다 다르다(Fluent 의 기본 갈래로 뭉치지 않았다).
  const labels = new Set(
    (["loginShell", "notInstalled", "notTrusted", "failed"] as const).map(T.fallback),
  );
  expect(labels.size).toBe(4);
});
