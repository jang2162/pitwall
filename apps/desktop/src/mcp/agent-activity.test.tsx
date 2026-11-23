// @vitest-environment happy-dom
// 「에이전트 활동」 툴 윈도우(16-m5-plan M5-13): 프로젝트 창(App kind="project", `?project=p1`)에서 상태바 「MCP n」 을 누르면 열린다.
// 가짜 Host 가 감사 로그(`mcp.audit.list` 의 쪽 규칙 — 그 프로젝트 + 프로젝트 없는 행) · `mcp.audit.clear` · 이벤트(`mcp.activity`)를
// 흉내 낸다. 완료 기준: 5,000행에서 DOM 줄 수가 화면 몫, 거르기 셋, 새 이벤트가 맨 위, 지우기는 확인을 거침, 결과가 글자로 보이고
// `started` 는 「끝을 모름」. 그리고 창이 앞으로 올 때 다시 읽기(carry-notes R8), 프로젝트 없는 행의 표지(R6).

import type {
  AuditRow,
  ConfigView,
  HostInfo,
  KeymapFile,
  McpAuditListParams,
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
import { ACTIVITY_ROW_HEIGHT, WAKE_COALESCE_MS } from "./agent-activity";
import { AUDIT_PAGE } from "./audit-log";
import { mcpStrings } from "./strings";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const host = vi.hoisted(() => ({
  /** UI 언어(`i18n.get`). */
  language: "ko",
  sessions: [] as unknown[],
  /** 감사 로그(id 오름차순으로 넣는다). */
  audit: [] as AuditRow[],
  listCalls: [] as McpAuditListParams[],
  clearCalls: [] as unknown[],
  /** `events.subscribe` 에 답하기 직전에 부른다(구독 사이에 쓰인 행 흉내). */
  onSubscribe: (_params: { projectId?: string; topics: string[] }) => {},
  emitHost: (_topic: string, _payload: unknown) => {},
  emitProject: (_topic: string, _payload: unknown) => {},
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
  const page = (p: McpAuditListParams) => {
    const before = p.cursor == null ? Infinity : Number(p.cursor);
    const matching = host.audit
      .filter((r) => p.projectId == null || r.projectId === p.projectId || r.projectId === null)
      .filter((r) => p.outcome == null || r.outcome === p.outcome)
      .filter((r) => p.tool == null || r.tool === p.tool)
      .filter((r) => r.id < before)
      .sort((a, b) => b.id - a.id);
    const limit = p.limit ?? 100;
    const entries = matching.slice(0, limit);
    return {
      entries,
      nextCursor: matching.length > limit ? String(entries.at(-1)!.id) : null,
    };
  };
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
            return reply(i18nState(host.language));
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
            return reply(host.sessions);
          case "mcp.tools.list":
            return reply(
              ["fs_read", "git_status", "db_execute"].map((name) => ({ name })) as unknown[],
            );
          case "mcp.audit.list": {
            const p = m.params as McpAuditListParams;
            host.listCalls.push(p);
            return reply(page(p));
          }
          case "mcp.audit.clear": {
            const p = m.params as { projectId: string };
            host.clearCalls.push(p);
            const before = host.audit.length;
            host.audit = host.audit.filter((r) => r.projectId !== p.projectId);
            return reply({ deleted: before - host.audit.length });
          }
          case "events.subscribe": {
            const id = next++;
            host.onSubscribe(m.params as { projectId?: string; topics: string[] });
            subs.set(id, m.params as { projectId?: string; topics: string[] });
            return reply({ subscriptionId: id });
          }
          case "events.unsubscribe":
            return reply(null);
          default:
            return send({ id: m.id, error: { code: -32601, message: `no ${m.method}` } });
        }
      });
      const emit = (projectId: string | undefined) => (topic: string, payload: unknown) => {
        for (const [subscriptionId, s] of subs) {
          if (s.projectId !== projectId || !s.topics.includes(topic)) continue;
          send({ method: "events.event", params: { subscriptionId, topic, payload } });
        }
      };
      host.emitHost = emit(undefined);
      host.emitProject = emit("p1");
      return { transport: client, close: async () => {} };
    },
  };
});

const T = mcpStrings(bundledTranslator("ko"));

function session(id: string, extra: Partial<McpSessionInfo> = {}) {
  return {
    id,
    client: "claude-code 2.1.0",
    projectId: "p1",
    transport: "socket",
    startedAt: Date.UTC(2026, 10, 17, 1, 2),
    calls: 0,
    ...extra,
  } satisfies McpSessionInfo;
}

function row(id: number, extra: Partial<AuditRow> = {}): AuditRow {
  return {
    id,
    at: Date.UTC(2026, 10, 17, 1, 0, 0) + id * 1000,
    session: "s1",
    client: "claude-code 2.1.0",
    projectId: "p1",
    tool: "fs_read",
    args: `{"path":"src/f${id}.rs"}`,
    outcome: "ok",
    detail: null,
    resultSize: 10,
    durationMs: 12,
    approval: null,
    planHash: null,
    transport: "socket",
    ...extra,
  };
}

const keymapNative: KeymapNative = {
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
  host.language = "ko";
  host.sessions = [session("a", { calls: 4 })];
  host.audit = [];
  host.listCalls = [];
  host.clearCalls = [];
  host.onSubscribe = () => {};
  localStorage.clear();
  // 호출 로그 표(role=table)가 400 px 높이다 — 보이는 줄 수가 정해진다.
  for (const prop of ["offsetHeight", "clientHeight"] as const)
    vi.spyOn(HTMLElement.prototype, prop, "get").mockImplementation(function (this: HTMLElement) {
      return this.getAttribute("role") === "table" ? 400 : 0;
    });
});

afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 20)));

async function mount() {
  window.history.replaceState(null, "", "/?window=project&project=p1");
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
    // StrictMode 로 감싸는 것은 시험 셋업이 한다(src/test-support/strict-mode.ts).
    root!.render(
      <ThemeStoreProvider store={store}>
        <App kind="project" native={keymapNative} />
      </ThemeStoreProvider>,
    ),
  );
  await settle();
}

const panel = () => document.querySelector<HTMLElement>('[data-tool-window="agent"]');
const table = () => panel()!.querySelector<HTMLElement>("[data-testid=mcp-log-rows]")!;
const logRows = () => [...panel()!.querySelectorAll<HTMLElement>("[data-mcp-row]")];
const rowIds = () => logRows().map((r) => Number(r.dataset.mcpRow));

/** 상태바 「MCP n」 을 눌러 패널을 연다(M5-10 ⑤ — M5-13 부터는 팝업이 아니라 이 툴 윈도우). */
async function openPanel() {
  await mount();
  const item = await vi.waitFor(() => {
    const el = document.querySelector<HTMLElement>("[data-testid=status-mcp]");
    expect(el).not.toBeNull();
    return el!;
  });
  await act(async () => item.click());
  await settle();
  await vi.waitFor(() => expect(panel()).not.toBeNull());
  await settle();
}

async function choose(filter: "outcome" | "tool" | "client", value: string) {
  const select = panel()!.querySelector<HTMLSelectElement>(`[data-mcp-filter="${filter}"]`)!;
  await act(async () => {
    select.value = value;
    select.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await settle();
}

it("opens from the status bar with the pending-approvals slot, the connected agents and the call log", async () => {
  host.audit = [row(1), row(2, { projectId: null, client: "pitwall-cli", tool: "git_status" })];
  await openPanel();
  expect(document.querySelector("[data-testid=mcp-sessions-popup]")).toBeNull();
  const sections = [...panel()!.querySelectorAll("section")].map((s) => s.dataset.testid);
  expect(sections).toEqual(["mcp-approvals", "mcp-agents", "mcp-call-log"]);
  expect(panel()!.querySelector("[data-testid=mcp-approvals-empty]")?.textContent).toBe(
    T.activity.approvalsEmpty,
  );
  const agent = panel()!.querySelector<HTMLElement>("[data-testid=mcp-session]")!;
  expect(agent.textContent).toContain("claude-code 2.1.0");
  expect(agent.textContent).toContain(T.sessions.calls(4));
  // R6: 이 로그가 무엇을 보이는지 적혀 있고, 프로젝트 없는 행에는 표지가 있다.
  expect(panel()!.querySelector("[data-testid=mcp-log-scope]")?.textContent).toBe(
    T.activity.logScope,
  );
  expect(host.listCalls[0]).toMatchObject({ projectId: "p1", cursor: null, limit: AUDIT_PAGE });
  expect(rowIds()).toEqual([2, 1]);
  const [cli, mine] = logRows();
  expect(cli!.querySelector("[data-mcp-no-project]")?.textContent).toBe(T.activity.noProject);
  expect(cli!.textContent).toContain("pitwall-cli");
  expect(mine!.querySelector("[data-mcp-no-project]")).toBeNull();
});

it("leaves Loading once the first page comes, with or without rows (M5-13b)", async () => {
  // 실기에서 표 머리 아래 「불러오는 중」 한 줄만 남았다 — 첫 쪽이 와도 그 자리를 지나지 못했다.
  await openPanel();
  const empty = () => panel()!.querySelector("[data-testid=mcp-log-empty]")?.textContent;
  expect(empty()).toBe(T.activity.empty);
  expect(table().textContent).not.toContain(T.activity.loading);
  act(() => root?.unmount());
  host.audit = [row(1, { projectId: null, client: "pitwall-cli", tool: "git_status" })];
  await openPanel();
  expect(rowIds()).toEqual([1]);
  expect(panel()!.querySelector("[data-testid=mcp-log-empty]")).toBeNull();
});

it.each(["ko", "en"])(
  "writes times in the UI language (%s), not the system locale (M5-13b)",
  async (language) => {
    // 실기에서 영어 UI 에 「오전 07:20」 이 나왔다 — 시각이 시스템 로캘을 따랐다. 두 언어로 보면 시스템 로캘이 무엇이든 하나는
    // 시스템과 다르다.
    host.language = language;
    const at = Date.UTC(2026, 10, 17, 1, 0, 0) + 7 * 1000;
    host.audit = [row(7)];
    await openPanel();
    const L = mcpStrings(bundledTranslator(language));
    const started = new Date(session("a").startedAt).toLocaleTimeString(language, {
      hour: "2-digit",
      minute: "2-digit",
    });
    const agent = panel()!.querySelector<HTMLElement>("[data-testid=mcp-session]")!;
    expect(agent.textContent).toContain(L.sessions.started(started));
    const time = logRows()[0]!.querySelector<HTMLElement>("[role=cell]")!;
    expect(time.textContent).toBe(
      new Date(at).toLocaleTimeString(language, {
        hour: "2-digit",
        minute: "2-digit",
        second: "2-digit",
      }),
    );
    expect(time.title).toBe(new Date(at).toLocaleString(language));
  },
);

it("draws only the visible rows of a 5,000-row log, reading it page by page", async () => {
  for (let id = 1; id <= 5_000; id++) host.audit.push(row(id));
  await openPanel();
  const scroller = table();
  // 다 읽기 전에는 전체 줄 수를 모른다(R3).
  expect(scroller.getAttribute("aria-rowcount")).toBe("-1");
  expect(scroller.dataset.loadedRows).toBe(String(AUDIT_PAGE));
  // 끝까지 스크롤해 쪽을 모두 읽는다.
  for (let i = 0; i < 40; i++) {
    const loaded = Number(scroller.dataset.loadedRows);
    if (loaded >= 5_000) break;
    await act(async () => {
      scroller.scrollTop = loaded * ACTIVITY_ROW_HEIGHT;
      scroller.dispatchEvent(new Event("scroll"));
    });
    await settle();
  }
  expect(Number(scroller.getAttribute("aria-rowcount"))).toBe(5_001);
  // 다음 쪽은 겹침 없이 앞 쪽의 마지막 id 를 커서로, 한 쪽씩 한 번만. 커서 없는 읽기는 첫 쪽과 구독이 열린 뒤의 한 번이고,
  // StrictMode(dev)가 effect 를 정리했다 다시 걸어 첫 쪽을 한 번 더 읽는다(정리된 쪽의 응답은 버린다 — M5-13b).
  const cursors = host.listCalls.map((c) => c.cursor);
  expect(cursors.filter((c) => c !== null)).toEqual(
    Array.from({ length: 5_000 / AUDIT_PAGE - 1 }, (_, i) =>
      String(5_000 - (i + 1) * AUDIT_PAGE + 1),
    ),
  );
  expect(cursors.filter((c) => c === null).length).toBeLessThanOrEqual(3);
  const body = scroller.querySelector<HTMLElement>("[role=rowgroup][style]")!;
  expect(body.style.height).toBe(`${5_000 * ACTIVITY_ROW_HEIGHT}px`);
  // 400 px / 22 px + 앞뒤 여분만.
  expect(logRows().length).toBeGreaterThan(0);
  expect(logRows().length).toBeLessThanOrEqual(45);
});

it("shows the result in words, with started rows as end unknown, the time taken and the approval", async () => {
  host.audit = [
    row(1, { outcome: "ok", durationMs: 12 }),
    row(2, { outcome: "error", detail: "no such file" }),
    row(3, { outcome: "denied", tool: "db_execute" }),
    row(4, { outcome: "started", durationMs: 0, approval: "once", tool: "fs_write" }),
    row(5, { approval: "window", client: "pitwall-cli" }),
    row(6, { approval: "auto" }),
    row(7, { approval: "session" }),
    row(8, { approval: "policy", tool: "run_start" }),
  ];
  await openPanel();
  const byId = (id: number) => panel()!.querySelector<HTMLElement>(`[data-mcp-row="${id}"]`)!;
  const outcome = (id: number) => byId(id).querySelector("[data-mcp-outcome]")!.textContent;
  const approval = (id: number) => byId(id).querySelector("[data-mcp-approval]")!.textContent;
  expect(outcome(1)).toBe("성공");
  expect(outcome(2)).toBe("실패");
  expect(outcome(3)).toBe("거절");
  expect(outcome(4)).toBe("끝을 모름");
  expect([1, 2, 3, 4].map(outcome)).toEqual(
    (["ok", "error", "denied", "started"] as const).map((o) => T.activity.outcome[o]),
  );
  expect(byId(1).textContent).toContain(T.activity.duration(12));
  // 끝을 모르는 행은 걸린 시간도 모른다(0 ms 가 아니다).
  expect(byId(4).textContent).not.toContain(T.activity.duration(0));
  expect(approval(4)).toBe(T.activity.approval.once);
  expect(approval(5)).toBe(T.activity.approval.window);
  expect(approval(6)).toBe(T.activity.approval.auto);
  expect(approval(7)).toBe(T.activity.approval.session);
  expect(approval(8)).toBe("규칙");
  expect(approval(8)).toBe(T.activity.approval.policy);
  expect(approval(1)).toBe(T.activity.approvalNone);
  expect(byId(2).querySelector("[data-mcp-outcome]")?.getAttribute("title")).toBe("no such file");
});

it("puts a new call on top from mcp.activity and updates a started row when it ends", async () => {
  host.audit = [row(1), row(2, { outcome: "started", durationMs: 0 })];
  await openPanel();
  expect(rowIds()).toEqual([2, 1]);
  host.audit.push(row(3, { tool: "git_status" }));
  await act(async () => host.emitProject("mcp.activity", { id: 3, projectId: "p1" }));
  await settle();
  expect(rowIds()).toEqual([3, 2, 1]);
  // 프로젝트 없는 호출은 Host 범위로 온다.
  host.audit.push(row(4, { projectId: null }));
  await act(async () => host.emitHost("mcp.activity", { id: 4, projectId: null }));
  await settle();
  expect(rowIds()).toEqual([4, 3, 2, 1]);
  // 선기록된 행의 끝 — 같은 id 로 한 번 더.
  host.audit[1] = row(2, { outcome: "ok", durationMs: 30 });
  await act(async () => host.emitProject("mcp.activity", { id: 2, projectId: "p1" }));
  await settle();
  expect(rowIds()).toEqual([4, 3, 2, 1]);
  expect(panel()!.querySelector('[data-mcp-row="2"] [data-mcp-outcome]')?.textContent).toBe(
    T.activity.outcome.ok,
  );
});

it("reads again when the window comes forward, for rows another process wrote (R8)", async () => {
  host.audit = [row(1), row(2, { outcome: "started", client: "pitwall-cli", durationMs: 0 })];
  await openPanel();
  // `pitwall tool` 이 쓴 행 — 이벤트가 없다.
  host.audit.push(row(3, { client: "pitwall-cli", tool: "git_status", projectId: "p1" }));
  host.audit[1] = row(2, { outcome: "error", client: "pitwall-cli" });
  await settle();
  expect(rowIds()).toEqual([2, 1]);
  await act(async () => window.dispatchEvent(new Event("focus")));
  await vi.waitFor(() => expect(rowIds()).toEqual([3, 2, 1]));
  expect(panel()!.querySelector('[data-mcp-row="3"]')?.textContent).toContain("pitwall-cli");
  expect(panel()!.querySelector('[data-mcp-row="2"] [data-mcp-outcome]')?.textContent).toBe(
    T.activity.outcome.error,
  );
  // 보이지 않다가 보이게 될 때도.
  host.audit.push(row(4));
  await act(async () => document.dispatchEvent(new Event("visibilitychange")));
  await vi.waitFor(() => expect(rowIds()).toEqual([4, 3, 2, 1]));
});

it("filters by result and tool on the host and by client in the panel", async () => {
  host.audit = [
    row(1, { outcome: "ok", tool: "fs_read" }),
    row(2, { outcome: "denied", tool: "db_execute" }),
    row(3, { outcome: "ok", tool: "git_status", client: "pitwall-cli" }),
    row(4, { outcome: "error", tool: "git_status", client: null }),
  ];
  await openPanel();
  expect(rowIds()).toEqual([4, 3, 2, 1]);

  await choose("outcome", "denied");
  expect(host.listCalls.at(-1)).toMatchObject({ outcome: "denied", tool: null, cursor: null });
  expect(rowIds()).toEqual([2]);
  await choose("outcome", "");

  await choose("tool", "git_status");
  expect(host.listCalls.at(-1)).toMatchObject({ outcome: null, tool: "git_status" });
  expect(rowIds()).toEqual([4, 3]);
  await choose("tool", "");
  expect(rowIds()).toEqual([4, 3, 2, 1]);

  const calls = host.listCalls.length;
  await choose("client", "pitwall-cli");
  expect(rowIds()).toEqual([3]);
  // 클라이언트 모름(`client: null`)도 고를 수 있다.
  const clientSelect = panel()!.querySelector<HTMLSelectElement>('[data-mcp-filter="client"]')!;
  expect([...clientSelect.options].map((o) => o.textContent)).toContain(T.sessions.unknownClient);
  await choose("client", "\u0000");
  expect(rowIds()).toEqual([4]);
  // 받은 행에서 거른다 — 끝까지 받았으면 다시 묻지 않는다.
  expect(host.listCalls.length).toBe(calls);
  await choose("outcome", "ok");
  expect(rowIds()).toEqual([]);
  expect(panel()!.querySelector("[data-testid=mcp-log-empty]")?.textContent).toBe(
    T.activity.emptyFiltered,
  );
});

it("reads more pages while the client filter leaves too few rows to fill the view", async () => {
  // 최신 600행은 다른 클라이언트, 그 아래에 고를 클라이언트의 행.
  for (let id = 1; id <= 5; id++) host.audit.push(row(id, { client: "pitwall-cli" }));
  for (let id = 6; id <= 605; id++) host.audit.push(row(id));
  host.sessions = [session("a"), session("b", { client: "pitwall-cli" })];
  await openPanel();
  await choose("client", "pitwall-cli");
  await vi.waitFor(() => expect(rowIds()).toEqual([5, 4, 3, 2, 1]));
  expect(host.listCalls.map((c) => c.cursor).filter((c) => c !== null)).toEqual([
    "406",
    "206",
    "6",
  ]);
});

it("clears the log only after confirmation, keeping rows without a project", async () => {
  host.audit = [row(1), row(2, { projectId: null }), row(3)];
  await openPanel();
  const clearButton = panel()!.querySelector<HTMLElement>("[data-testid=mcp-clear]")!;
  expect(clearButton.textContent).toBe(T.activity.clear);
  await act(async () => clearButton.click());
  await settle();
  const dialog = () => document.querySelector<HTMLElement>("[data-testid=mcp-clear-confirm]");
  expect(dialog()?.textContent).toContain(T.activity.clearTitle);
  // 처음 포커스는 [취소].
  expect(document.activeElement?.textContent).toBe(T.activity.clearCancel);
  await act(async () => (document.activeElement as HTMLElement).click());
  await settle();
  expect(dialog()).toBeNull();
  expect(host.clearCalls).toEqual([]);
  expect(rowIds()).toEqual([3, 2, 1]);

  // Esc 로 닫아도 지우지 않는다(M5-13 리뷰 F4 — 되돌릴 수 없는 삭제가 닫기 길로 새지 않게).
  await act(async () => clearButton.click());
  await settle();
  expect(dialog()).not.toBeNull();
  await act(async () => {
    document.activeElement!.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
    );
  });
  await settle();
  expect(dialog()).toBeNull();
  expect(host.clearCalls).toEqual([]);
  expect(rowIds()).toEqual([3, 2, 1]);

  await act(async () => clearButton.click());
  await settle();
  await act(async () =>
    document.querySelector<HTMLElement>("[data-testid=mcp-clear-confirm-button]")!.click(),
  );
  await settle();
  expect(host.clearCalls).toEqual([{ projectId: "p1" }]);
  expect(dialog()).toBeNull();
  expect(rowIds()).toEqual([2]);
  // 지운 뒤에도 새 행은 맨 위에(id 는 다시 쓰이지 않는다 — M5-11).
  host.audit.push(row(4));
  await act(async () => host.emitProject("mcp.activity", { id: 4, projectId: "p1" }));
  await settle();
  expect(rowIds()).toEqual([4, 2]);
});

it("refreshes the call counts of the connected agents on mcp.activity", async () => {
  await openPanel();
  host.sessions = [session("a", { calls: 9 })];
  host.audit.push(row(1));
  await act(async () => host.emitProject("mcp.activity", { id: 1, projectId: "p1" }));
  await settle();
  expect(panel()!.querySelector("[data-testid=mcp-session]")?.textContent).toContain(
    T.sessions.calls(9),
  );
});

it("shows control and bidi characters agents sent as replacement marks", async () => {
  host.audit = [
    row(1, { args: '{"path":"a\u202Egnp.exe","b":"\u2066x\u2069"}', client: "evil\u0007 1.0" }),
  ];
  await openPanel();
  const text = logRows()[0]!.textContent!;
  expect(/[\u202E\u2066\u2069]/u.test(text) || text.includes("\u0007")).toBe(false);
  expect(text).toContain('"\uFFFDx\uFFFD"');
  expect(text).toContain("a\uFFFDgnp.exe");
  expect(text).toContain("evil\uFFFD 1.0");
});

const listCallsAfter = async (act_: () => Promise<void>) => {
  const before = host.listCalls.length;
  await act_();
  // 묶는 간격이 지나고 읽기가 끝날 때까지.
  await act(async () => new Promise((r) => setTimeout(r, WAKE_COALESCE_MS * 2)));
  await settle();
  return host.listCalls.slice(before);
};

const wakeWindow = () =>
  act(async () => {
    window.dispatchEvent(new Event("focus"));
    document.dispatchEvent(new Event("visibilitychange"));
  });

async function scrollTo(top: number) {
  await act(async () => {
    table().scrollTop = top;
    table().dispatchEvent(new Event("scroll"));
  });
  await settle();
}

it("reads one page when the window comes forward, not down to an old started row (F3)", async () => {
  // 리뷰어의 측정: 1,000행을 모두 받아 둔 뒤 id 5 가 started 로 남은 경우 — 포커스 한 번에 5쪽, focus · visibilitychange 가
  // 함께면 10쪽이었다.
  for (let id = 1; id <= 1_000; id++)
    host.audit.push(row(id, id === 5 ? { outcome: "started", durationMs: 0 } : {}));
  await openPanel();
  for (let i = 0; i < 10 && Number(table().dataset.loadedRows) < 1_000; i++)
    await scrollTo(Number(table().dataset.loadedRows) * ACTIVITY_ROW_HEIGHT);
  expect(Number(table().getAttribute("aria-rowcount"))).toBe(1_001);
  await scrollTo(0);
  const calls = await listCallsAfter(wakeWindow);
  expect(calls.map((c) => c.cursor)).toEqual([null]);

  // 그 started 행이 화면에 있으면 그 행만 id 로 다시 읽는다.
  host.audit[4] = row(5, { outcome: "error" });
  await scrollTo(990 * ACTIVITY_ROW_HEIGHT);
  const again = await listCallsAfter(wakeWindow);
  expect(again.map((c) => [c.cursor, c.limit])).toEqual([
    [null, AUDIT_PAGE],
    ["6", 1],
  ]);
  expect(panel()!.querySelector('[data-mcp-row="5"] [data-mcp-outcome]')?.textContent).toBe(
    T.activity.outcome.error,
  );
});

it("does not read while the panel is collapsed, and reads once when it shows again (F3)", async () => {
  host.audit = [row(1)];
  await openPanel();
  const status = document.querySelector<HTMLElement>("[data-testid=status-mcp]")!;
  // 열려 있는 툴 윈도우의 열기 액션은 그 dock 을 접는다.
  await act(async () => status.click());
  await settle();
  host.audit.push(row(2));
  const hidden = await listCallsAfter(async () => {
    await wakeWindow();
    await act(async () => host.emitProject("mcp.activity", { id: 2, projectId: "p1" }));
  });
  expect(hidden).toEqual([]);
  const shown = await listCallsAfter(async () => {
    await act(async () => status.click());
  });
  expect(shown.map((c) => c.cursor)).toEqual([null]);
  expect(rowIds()).toEqual([2, 1]);
});

it("keeps the row being read in place when new calls arrive above it, and shows them at the top (R1)", async () => {
  for (let id = 1; id <= 100; id++) host.audit.push(row(id));
  await openPanel();
  await scrollTo(10 * ACTIVITY_ROW_HEIGHT);
  const firstVisible = () => {
    const top = table().scrollTop;
    return logRows().find((r) => {
      const y = Number(/translateY\((\d+)px\)/.exec(r.style.transform)?.[1]);
      return y === top;
    })?.dataset.mcpRow;
  };
  expect(firstVisible()).toBe("90");
  host.audit.push(row(101), row(102), row(103));
  await act(async () => host.emitProject("mcp.activity", { id: 101, projectId: "p1" }));
  await act(async () => host.emitProject("mcp.activity", { id: 103, projectId: "p1" }));
  await settle();
  expect(table().scrollTop).toBe(13 * ACTIVITY_ROW_HEIGHT);
  expect(firstVisible()).toBe("90");

  // 맨 위에 있으면 새 행이 그대로 보인다.
  await scrollTo(0);
  host.audit.push(row(104));
  await act(async () => host.emitProject("mcp.activity", { id: 104, projectId: "p1" }));
  await settle();
  expect(table().scrollTop).toBe(0);
  expect(firstVisible()).toBe("104");
});

it("reads once more when the subscription opens, for rows written between the first page and it", async () => {
  host.audit = [row(1)];
  // 첫 쪽에 답한 뒤 · 구독이 열리기 전에 쓰인 행 — 이벤트가 오지 않는다.
  host.onSubscribe = (p) => {
    // 창의 다른 기능도 프로젝트 범위로 구독한다 — 이 패널의 `mcp.activity` 구독일 때만.
    if (p.projectId === "p1" && p.topics.includes("mcp.activity") && host.audit.length === 1)
      host.audit.push(row(2));
  };
  await openPanel();
  await vi.waitFor(() => expect(rowIds()).toEqual([2, 1]));
});
