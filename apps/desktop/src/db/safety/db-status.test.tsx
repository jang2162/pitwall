// @vitest-environment happy-dom
// 상태바 `db` 항목(M3-19 ①): 프로젝트 창(App kind="project", `?project=p1`)을 가짜 Host 에 붙이고, 지금 탭의 세션을
// `setActiveDbSession`(콘솔 탭 M3-15 의 자리)으로 둔 뒤 Host 가 `db.sessionChanged` 를 내면 그 세션의 트랜잭션 시간 · `failed` ·
// 끊김을 따라가는지 본다. 연결 이름 · 프로덕션 라벨이 색과 함께 보인다. 열린 시간은 초마다 다시 그린다.

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
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, vi } from "vitest";

import { act, endTest, it } from "../../test-support/ended-test";
import defaultTheme from "../../../../../extensions/theme-default/themes/default.json";
import { App } from "../../App";
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
  setActiveDbSession,
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

function active(state: DbSessionInfo["state"], over: Partial<DbActiveSession> = {}) {
  return {
    session: session(state),
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
  setActiveDbSession(undefined);
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

async function mountApp() {
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

describe("the status bar's db item in a project window", () => {
  it("follows db.sessionChanged for the current tab's session: open time, failed, disconnected", async () => {
    vi.spyOn(Date, "now").mockReturnValue(NOW);
    await mountApp();
    // DB 탭이 앞에 없으면 그리지 않는다.
    expect(item()).toBeNull();
    expect(host.subscribed).toContainEqual({ projectId: "p1", topics: ["db.sessionChanged"] });

    await act(async () => setActiveDbSession(active({ kind: "idle" })));
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

    // 탭이 뒤로 가면 항목이 사라진다.
    await act(async () => setActiveDbSession(undefined));
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
    setActiveDbSession(
      active({ kind: "inTransaction", since: NOW - 59_000 }, { production: false }),
    );
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

  it("applies events only to the current tab's session", async () => {
    setActiveDbSession(active({ kind: "idle" }));
    applyDbSessionChanged(changed(session({ kind: "disconnected" }, { sessionId: "other" })));
    expect(dbStatusStore.getState().active!.session.state.kind).toBe("idle");
    applyDbSessionChanged(changed(session({ kind: "failed", since: 1 })));
    expect(dbStatusStore.getState().active!.session.state).toEqual({ kind: "failed", since: 1 });
    setActiveDbSession(undefined);
    applyDbSessionChanged(changed(session({ kind: "idle" })));
    expect(dbStatusStore.getState().active).toBeUndefined();
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
