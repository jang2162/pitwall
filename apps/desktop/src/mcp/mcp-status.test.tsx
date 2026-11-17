// @vitest-environment happy-dom
// MCP 상태 표시(16-m5-plan M5-10 ⑤): 프로젝트 창(App kind="project", `?project=p1`)의 상태바 「MCP n」 — 이 프로젝트를 세션
// 프로젝트로 가진 세션 수, 0 이면 항목이 없다, 누르면 「에이전트 활동」 툴 윈도우(M5-13 — 그 전에는 세션 목록 팝업). 세션이 열리고 닫히면(`mcp.sessionsChanged`, Host 범위)
// 다시 읽는다. 웰컴 창(App kind="welcome")의 아래 줄 「MCP: 연결 n」 은 세션 전부의 수다.

import type {
  ConfigView,
  HostInfo,
  KeymapFile,
  McpSessionInfo,
  Project,
  ProjectTrust,
} from "@pitwall/rpc-client";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import { bundledTranslator } from "../i18n/testing";
import type { KeymapNative } from "../keymap/native";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import type { WelcomeNative } from "../welcome/native";
import { mcpStrings } from "./strings";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const host = vi.hoisted(() => ({
  /** `mcp.sessions` 의 응답. */
  sessions: [] as unknown[],
  reads: 0,
  /** Host 범위(프로젝트 없이)로 구독한 창에 이벤트를 보낸다. 연결마다 바뀐다. */
  emitHost: (_topic: string, _payload: unknown) => {},
}));

vi.mock("../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../keymap/default-keymap");
  const { i18nState } = await import("../i18n/testing");
  const project = {
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
  } satisfies Project;
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
            return reply(project);
          case "project.list":
            return reply([project]);
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
          case "mcp.sessions":
            host.reads += 1;
            return reply(host.sessions);
          case "mcp.tools.list":
            return reply([]);
          case "mcp.audit.list":
            return reply({ entries: [], nextCursor: null });
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
      host.emitHost = (topic, payload) => {
        for (const [subscriptionId, s] of subs) {
          if (s.projectId !== undefined || !s.topics.includes(topic)) continue;
          send({ method: "events.event", params: { subscriptionId, topic, payload } });
        }
      };
      return { transport: client, close: async () => {} };
    },
  };
});

const T = mcpStrings(bundledTranslator("ko"));

function session(id: string, projectId: string | null, extra: Partial<McpSessionInfo> = {}) {
  return {
    id,
    client: "claude-code 2.1.0",
    projectId,
    transport: "socket",
    startedAt: Date.UTC(2026, 10, 17, 1, 2),
    calls: 0,
    ...extra,
  } satisfies McpSessionInfo;
}

const keymapNative: KeymapNative = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
  closeWindow: async () => {},
};

const welcomeNative: WelcomeNative = {
  openProjectWindow: async () => {},
  openFileWindow: async () => {},
  takePendingFindProject: async () => null,
  onFindProjectRequested: () => () => {},
  pickFolder: async () => null,
  reveal: async () => {},
  onDragDrop: () => () => {},
  pathKind: async () => "dir",
};

let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(console, "info").mockImplementation(() => {});
  vi.spyOn(console, "warn").mockImplementation(() => {});
  host.sessions = [];
  host.reads = 0;
  localStorage.clear();
});

afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 20)));

async function mount(kind: "project" | "welcome") {
  window.history.replaceState(
    null,
    "",
    kind === "project" ? "/?window=project&project=p1" : "/?window=welcome",
  );
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
        {kind === "project" ? (
          <App kind="project" native={keymapNative} />
        ) : (
          <App kind="welcome" native={keymapNative} welcomeNative={welcomeNative} />
        )}
      </ThemeStoreProvider>,
    ),
  );
  await settle();
}

const item = () => document.querySelector<HTMLElement>("[data-testid=status-mcp]");

/** 상태바 왼쪽 항목 id(화면 순서) — 가운데 빈칸(`span`) 앞까지. */
function leftItems(): string[] {
  const footer = item()!.closest("footer")!;
  const ids: string[] = [];
  for (const el of footer.children) {
    const id = (el as HTMLElement).dataset.statusItem;
    if (id === undefined) break;
    ids.push(id);
  }
  return ids;
}

it("shows the number of sessions on this project, and nothing at zero", async () => {
  host.sessions = [session("a", "p1"), session("b", "p1"), session("c", "p2"), session("d", null)];
  await mount("project");
  expect(item()?.textContent).toBe(T.status.label(2));
  expect(item()?.textContent).toBe("MCP 2");
  // 상태바의 `mcp` 자리(`STATUS_SLOTS.mcp`) — 왼쪽, Host 항목 다음 · 신뢰 항목 앞.
  const slot = item()!.closest<HTMLElement>("[data-status-item]");
  expect(slot?.dataset.statusItem).toBe("mcp");
  expect(leftItems().join(" ")).toContain("host mcp trust");

  // 세션이 모두 닫히면 `mcp.sessionsChanged` 로 다시 읽고 항목이 사라진다.
  host.sessions = [session("c", "p2")];
  await act(async () => host.emitHost("mcp.sessionsChanged", { total: 1 }));
  await settle();
  expect(item()).toBeNull();
  // 자리는 남고 비어 숨는다(`empty:hidden` — status-bar.tsx).
  expect(document.querySelector('[data-status-item="mcp"]')?.childElementCount).toBe(0);
});

it("has no item when no session is on this project", async () => {
  host.sessions = [session("c", "p2")];
  await mount("project");
  expect(host.reads).toBeGreaterThan(0);
  expect(item()).toBeNull();
});

it("opens the Agent Activity tool window listing the project's sessions when clicked", async () => {
  host.sessions = [
    session("a", "p1", { calls: 3 }),
    session("b", "p1", { client: null }),
    session("c", "p2"),
  ];
  await mount("project");
  expect(item()?.getAttribute("title")).toBe(T.status.hint(2));
  await act(async () => item()!.click());
  await settle();
  const panel = await vi.waitFor(() => {
    const el = document.querySelector<HTMLElement>('[data-tool-window="agent"]');
    expect(el).not.toBeNull();
    return el!;
  });
  expect(document.querySelector("[data-testid=mcp-sessions-popup]")).toBeNull();
  const agents = panel.querySelector<HTMLElement>("[data-testid=mcp-agents]")!;
  expect(agents.textContent).toContain(T.sessions.title);
  const rows = [...agents.querySelectorAll<HTMLElement>("[data-testid=mcp-session]")];
  expect(rows).toHaveLength(2);
  expect(rows[0]!.textContent).toContain("claude-code 2.1.0");
  expect(rows[0]!.textContent).toContain(T.sessions.calls(3));
  expect(rows[1]!.textContent).toContain(T.sessions.unknownClient);

  // 마지막 세션이 닫히면 상태바 항목은 사라지고 패널은 빈 목록을 보이며 남는다 — 「MCP 0」 은 없다.
  host.sessions = [];
  await act(async () => host.emitHost("mcp.sessionsChanged", { total: 0 }));
  await settle();
  expect(item()).toBeNull();
  expect(agents.textContent).toContain(T.sessions.empty);
  expect(document.body.textContent).not.toContain(T.status.label(0));
});

it("counts every session in the welcome window", async () => {
  host.sessions = [session("a", "p1"), session("b", "p2"), session("c", null)];
  await mount("welcome");
  const line = () => document.querySelector<HTMLElement>("[data-testid=welcome-mcp]");
  expect(line()?.textContent).toBe(T.welcome.connections(3));
  expect(line()?.textContent).toBe("MCP: 연결 3");

  host.sessions = [];
  await act(async () => host.emitHost("mcp.sessionsChanged", { total: 0 }));
  await settle();
  expect(line()?.textContent).toBe("MCP: 연결 0");
});
