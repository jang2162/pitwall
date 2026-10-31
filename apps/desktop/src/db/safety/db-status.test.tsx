// @vitest-environment happy-dom
// 상태바 `db` 항목(M3-19 ①): 프로젝트 창(App kind="project", `?project=p1`)을 가짜 Host 에 붙이고, 에디터 탭을 열어 그 탭의
// 세션을 `setDbTabSession`(콘솔 탭 M3-15 의 자리)으로 둔 뒤 Host 가 `db.sessionChanged` 를 내면 그 세션의 트랜잭션 시간 ·
// `failed` · 끊김을 따라가는지 본다. 항목은 앞에 온 탭의 세션만 보인다 — 탭 전환 · 탭 닫힘 · 세션 닫힘 뒤에 낡은 세션이 남지
// 않는다. 연결 이름 · 프로덕션 라벨이 색과 함께 보인다. 열린 시간은 초마다 다시 그린다.

import { LocalizationProvider } from "@fluent/react";
import type {
  ConfigView,
  DbSessionChanged,
  DbSessionInfo,
  HostInfo,
  KeymapFile,
  Project,
  ProjectTrust,
  RuntimeInfo,
} from "@pitwall/rpc-client";
import type { DockviewApi } from "dockview-react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, vi } from "vitest";

import { act, endTest, it } from "../../test-support/ended-test";
import defaultTheme from "../../../../../extensions/theme-default/themes/default.json";
import { App } from "../../App";
import { EDITOR_PANEL } from "../../editor/editor-tabs";
import { bundledLocalization, bundledTranslator } from "../../i18n/testing";
import type { KeymapNative } from "../../keymap/native";
import { createThemeStore } from "../../theme/theme-store";
import { ThemeStoreProvider } from "../../theme/theme-switcher";
import { parseTheme } from "../../theme/tokens";
import {
  applyDbSessionChanged,
  type DbActiveSession,
  DbStatusItem,
  dbStatusStore,
  describeSessionState,
  formatElapsed,
  frontDbSession,
  setDbFrontTab,
  setDbTabSession,
  useDbTabSession,
} from "./db-status";
import { dbSafetyStrings } from "./strings";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const host = vi.hoisted(() => ({
  emit: (_projectId: string, _topic: string, _payload: unknown) => {},
  subscribed: [] as { projectId?: string; topics: string[] }[],
}));

vi.mock("../../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../../keymap/default-keymap");
  const { i18nState } = await import("../../i18n/testing");
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
            return reply({
              provider: "shell",
              source: { kind: "loginShell", shell: "/bin/zsh" },
              status: { kind: "ok" },
              tools: [],
            } satisfies RuntimeInfo);
          case "events.subscribe": {
            const id = next++;
            const params = m.params as { projectId?: string; topics: string[] };
            subs.set(id, params);
            host.subscribed.push(params);
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
          send({ method: "events.event", params: { subscriptionId, projectId, topic, payload } });
        }
      };
      return { transport: client, close: async () => {} };
    },
  };
});

const T = dbSafetyStrings(bundledTranslator("ko"));
const NOW = 1_800_000_000_000;

const native: KeymapNative = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
  closeWindow: async () => {},
};

function session(state: DbSessionInfo["state"], over: Partial<DbSessionInfo> = {}): DbSessionInfo {
  return {
    sessionId: "s1",
    projectId: "p1",
    connectionId: "billing",
    mode: "manual",
    state,
    ...over,
  };
}

function active(
  state: DbSessionInfo["state"],
  over: Partial<DbActiveSession> = {},
  sessionOver: Partial<DbSessionInfo> = {},
): DbActiveSession {
  return {
    session: session(state, sessionOver),
    name: "prod-billing",
    production: true,
    readOnly: false,
    ...over,
  };
}

const changed = (s: DbSessionInfo): DbSessionChanged => ({ session: s, notice: null });

let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(console, "info").mockImplementation(() => {});
  vi.spyOn(console, "warn").mockImplementation(() => {});
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.subscribed = [];
  localStorage.clear();
  dbStatusStore.setState({ tabs: {}, frontTabId: undefined });
});

afterEach(async () => {
  await endTest();
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 20)));
const item = () => document.querySelector<HTMLElement>("[data-testid=status-db]");
const transaction = () =>
  document.querySelector<HTMLElement>("[data-testid=status-db-transaction]");

let dock: DockviewApi | undefined;

/** 에디터 탭 하나를 연다(앞에 온다). */
function openTab(id: string) {
  dock!.addPanel({ id, component: EDITOR_PANEL, params: { path: `/work/demo/${id}.sql` } });
}

async function mountApp() {
  dock = undefined;
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
        <App kind="project" native={native} onDockApi={(api) => (dock = api)} />
      </ThemeStoreProvider>,
    ),
  );
  await settle();
}

describe("the status bar's db item in a project window", () => {
  it("follows db.sessionChanged for the current tab's session: open time, failed, disconnected", async () => {
    vi.spyOn(Date, "now").mockReturnValue(NOW);
    await mountApp();
    // DB 탭이 앞에 없으면 그리지 않는다.
    expect(item()).toBeNull();
    expect(host.subscribed).toContainEqual({ projectId: "p1", topics: ["db.sessionChanged"] });

    await act(async () => {
      openTab("console");
      setDbTabSession("console", active({ kind: "idle" }));
    });
    await settle();
    // 항목은 상태바 안의 `db` 자리다.
    expect(item()!.closest<HTMLElement>("[data-status-item]")!.dataset.statusItem).toBe("db");
    expect(item()!.dataset.mode).toBe("manual");
    expect(item()!.textContent).toContain("prod-billing");
    // 색만이 아니라 라벨이 있다.
    expect(item()!.querySelector("[data-label=production]")!.textContent).toBe("프로덕션");
    expect(item()!.textContent).toContain(T.status.mode.manual);
    expect(transaction()).toBeNull();

    // 수동 트랜잭션이 65초 전에 열렸다.
    await act(async () =>
      host.emit(
        "p1",
        "db.sessionChanged",
        changed(session({ kind: "inTransaction", since: NOW - 65_000 })),
      ),
    );
    await settle();
    expect(item()!.dataset.state).toBe("inTransaction");
    expect(transaction()!.textContent).toBe("· TX 1:05");
    expect(transaction()!.dataset.warning).toBeUndefined();

    // 다른 세션 · 다른 프로젝트의 이벤트는 이 항목과 무관하다.
    await act(async () => {
      host.emit(
        "p1",
        "db.sessionChanged",
        changed(session({ kind: "disconnected" }, { sessionId: "s2" })),
      );
      host.emit("p2", "db.sessionChanged", changed(session({ kind: "disconnected" })));
    });
    await settle();
    expect(item()!.dataset.state).toBe("inTransaction");

    // 오류 뒤 롤백만 되는 트랜잭션.
    await act(async () =>
      host.emit(
        "p1",
        "db.sessionChanged",
        changed(session({ kind: "failed", since: NOW - 5_000 })),
      ),
    );
    await settle();
    expect(item()!.dataset.state).toBe("failed");
    expect(transaction()!.textContent).toBe("· TX 실패 0:05 — 롤백만 된다");
    expect(transaction()!.dataset.warning).toBe("true");

    await act(async () =>
      host.emit("p1", "db.sessionChanged", changed(session({ kind: "disconnected" }))),
    );
    await settle();
    expect(transaction()!.textContent).toBe(`· ${T.status.disconnected}`);

    // 세션을 닫으면 항목이 사라진다.
    await act(async () => setDbTabSession("console", undefined));
    expect(item()).toBeNull();
  });

  it("shows only the front tab's session through tab switches, closes and session closes", async () => {
    await mountApp();
    const shown = () => item()?.textContent ?? null;
    // 두 콘솔 탭과 DB 가 아닌 탭. 뒤의 탭이 먼저 등록해도(쓴 순서가 뒤바뀌어도) 앞에 온 탭의 것이 보인다.
    await act(async () => {
      openTab("a");
      openTab("b");
      setDbTabSession(
        "b",
        active({ kind: "idle" }, { name: "db-b", production: false }, { sessionId: "sb" }),
      );
      setDbTabSession(
        "a",
        active({ kind: "idle" }, { name: "db-a", production: false }, { sessionId: "sa" }),
      );
    });
    await settle();
    expect(shown()).toContain("db-b");

    // 탭 전환: 앞에 온 탭의 세션.
    await act(async () => dock!.getPanel("a")!.api.setActive());
    await settle();
    expect(shown()).toContain("db-a");
    // 뒤로 간 탭의 이벤트는 앞 탭의 항목을 바꾸지 않는다.
    await act(async () =>
      host.emit(
        "p1",
        "db.sessionChanged",
        changed(session({ kind: "disconnected" }, { sessionId: "sb" })),
      ),
    );
    await settle();
    expect(item()!.dataset.state).toBe("idle");

    // DB 가 아닌 탭이 앞에 오면 항목이 없다.
    await act(async () => openTab("plain"));
    await settle();
    expect(item()).toBeNull();

    // 앞에 온 탭이 닫히면 그 세션은 보이지 않는다 — 탭이 제 세션을 빼기 전이라도.
    await act(async () => dock!.getPanel("b")!.api.setActive());
    await settle();
    expect(shown()).toContain("db-b");
    expect(item()!.dataset.state).toBe("disconnected");
    await act(async () => dock!.removePanel(dock!.getPanel("b")!));
    await settle();
    expect(dbStatusStore.getState().frontTabId).not.toBe("b");
    expect(shown() ?? "").not.toContain("db-b");
    await act(async () => setDbTabSession("b", undefined));

    // 남은 콘솔 탭이 앞에 와 있는 동안 그 세션이 닫히면 항목이 사라진다.
    await act(async () => dock!.getPanel("a")!.api.setActive());
    await settle();
    expect(shown()).toContain("db-a");
    await act(async () => setDbTabSession("a", undefined));
    expect(item()).toBeNull();
    // 닫힌 세션의 이벤트가 늦게 와도 되살아나지 않는다.
    await act(async () =>
      host.emit("p1", "db.sessionChanged", changed(session({ kind: "idle" }, { sessionId: "sa" }))),
    );
    await settle();
    expect(item()).toBeNull();
  });
});

describe("DbStatusItem", () => {
  it("counts the open transaction's time every second", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
    vi.setSystemTime(NOW);
    const element = document.createElement("div");
    document.body.append(element);
    root = createRoot(element);
    setDbTabSession(
      "t1",
      active({ kind: "inTransaction", since: NOW - 59_000 }, { production: false }),
    );
    setDbFrontTab("t1");
    await act(async () =>
      root!.render(
        <LocalizationProvider l10n={bundledLocalization("ko")}>
          <DbStatusItem />
        </LocalizationProvider>,
      ),
    );
    expect(transaction()!.textContent).toBe("· TX 0:59");
    await act(async () => vi.advanceTimersByTime(1000));
    expect(transaction()!.textContent).toBe("· TX 1:00");
    await act(async () => vi.advanceTimersByTime(3600_000));
    expect(transaction()!.textContent).toBe("· TX 1:01:00");
    // 프로덕션이 아니면 라벨이 없다.
    expect(item()!.querySelector("[data-label]")).toBeNull();
  });

  it("applies events only to the tabs holding that session", async () => {
    setDbTabSession("t1", active({ kind: "idle" }));
    setDbTabSession("t2", active({ kind: "idle" }, {}, { sessionId: "s2" }));
    setDbFrontTab("t1");
    const front = () => frontDbSession(dbStatusStore.getState());
    applyDbSessionChanged(changed(session({ kind: "disconnected" }, { sessionId: "other" })));
    expect(front()!.session.state.kind).toBe("idle");
    applyDbSessionChanged(changed(session({ kind: "failed", since: 1 })));
    expect(front()!.session.state).toEqual({ kind: "failed", since: 1 });
    expect(dbStatusStore.getState().tabs.t2!.session.state.kind).toBe("idle");
    setDbTabSession("t1", undefined);
    applyDbSessionChanged(changed(session({ kind: "idle" })));
    expect(front()).toBeUndefined();
    expect(Object.keys(dbStatusStore.getState().tabs)).toEqual(["t2"]);
  });

  it("keeps a tab's session while the tab is mounted and drops it when the tab goes", async () => {
    function Tab({ id, value }: { id: string; value: DbActiveSession | undefined }) {
      useDbTabSession(id, value);
      return null;
    }
    const element = document.createElement("div");
    document.body.append(element);
    root = createRoot(element);
    const a = active({ kind: "idle" });
    const b = active({ kind: "idle" }, {}, { sessionId: "s2" });
    await act(async () =>
      root!.render(
        <>
          <Tab id="t1" value={a} />
          <Tab id="t2" value={b} />
        </>,
      ),
    );
    expect(dbStatusStore.getState().tabs).toEqual({ t1: a, t2: b });
    // 한 탭이 세션을 닫아도 다른 탭의 것은 남는다.
    await act(async () =>
      root!.render(
        <>
          <Tab id="t1" value={undefined} />
          <Tab id="t2" value={b} />
        </>,
      ),
    );
    expect(dbStatusStore.getState().tabs).toEqual({ t2: b });
    // 탭이 닫히면(언마운트) 그 탭의 것이 빠진다.
    await act(async () => root!.render(<Tab id="t1" value={a} />));
    expect(dbStatusStore.getState().tabs).toEqual({ t1: a });
    await act(async () => root!.render(<></>));
    expect(dbStatusStore.getState().tabs).toEqual({});
  });
});

describe("describeSessionState", () => {
  it("words each state", async () => {
    const d = (state: DbSessionInfo["state"]) => describeSessionState(state, NOW, T.status);
    expect(d({ kind: "idle" })).toBeUndefined();
    expect(d({ kind: "inTransaction", since: NOW - 1_000 })).toEqual({
      kind: "inTransaction",
      text: "TX 0:01",
      warning: false,
    });
    expect(d({ kind: "failed", since: NOW })).toMatchObject({
      warning: true,
      text: T.status.failed("0:00"),
    });
    expect(d({ kind: "disconnected" })).toMatchObject({
      warning: true,
      text: T.status.disconnected,
    });
    expect(d({ kind: "configChanged", transaction: null })?.text).toBe(T.status.configChanged);
    expect(
      d({ kind: "configChanged", transaction: { since: NOW - 120_000, failed: false } })?.text,
    ).toBe(`${T.status.configChanged} · ${T.status.transaction("2:00")}`);
    expect(d({ kind: "configChanged", transaction: { since: NOW, failed: true } })?.text).toBe(
      `${T.status.configChanged} · ${T.status.failed("0:00")}`,
    );
  });

  it("formats elapsed time", async () => {
    expect(formatElapsed(0)).toBe("0:00");
    expect(formatElapsed(-5_000)).toBe("0:00");
    expect(formatElapsed(9_999)).toBe("0:09");
    expect(formatElapsed(600_000)).toBe("10:00");
    expect(formatElapsed(3_723_000)).toBe("1:02:03");
  });
});
