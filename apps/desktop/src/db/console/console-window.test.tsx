// @vitest-environment happy-dom
// SQL 콘솔(14-m3-plan.md M3-15 의 완료 기준)을 프로젝트 창(App kind="project")에서 가짜 Host 로 본다: 커서가 둘째 문장 안이면 그
// 문장만 돈다(보내는 글은 `state.doc.toString()` — CRLF 가 아니다), 세 문장 선택 → 결과 탭 셋, `DELETE FROM t` 에 확인이 뜨고
// [취소] 면 실행 RPC 가 불리지 않으며 [실행] 이면 받은 `statementsHash` 가 그대로 실린다, 수동 커밋에서 [롤백] 이
// `db.session.rollback` 을 부른다, ⌘F2 가 `db.query.cancel` 을 부른다, 수동 트랜잭션이 열린 탭 닫기 · 창 닫기(앱 종료도 같은 길 —
// `guardClose`)가 묻고 [취소] 면 닫히지 않는다, 콘솔 탭이 열린 채 다시 시작하면 `db.session.open` 이 불리지 않는다(`resume` 만),
// `failed` 세션에서 [커밋] 이 꺼져 있다, 한글 IME 로 SQL 을 칠 수 있다(조합 중의 ⌘↩ 은 돌지 않는다). 그 밖에 실행 계획 트리,
// [연결] 이 `3101` 을 받으면 DB 접근 묻기(M3-13)로 비밀번호를 넣고 다시 연다, 읽지 못한 · 아직 읽지 않은 콘솔 파일은 쓰거나 지우지
// 않는다, 콘솔 쓰기는 하나씩 가고 마지막 글이 남는다, 연결 · 재개 중에 닫은 탭의 세션은 돌아오면 닫는다.
//
// Glide 는 canvas 라 happy-dom 에서 그릴 수 없어 기록하는 가짜로 바꾼다(결과 그리드 자체는 src/db/grid 의 시험이 본다).

import { EditorView } from "@codemirror/view";
import type {
  ConfigView,
  DbSessionInfo,
  DbSessionOpened,
  DbSqlClassification,
  HostInfo,
  KeymapFile,
  Project,
  ProjectTrust,
  RuntimeInfo,
} from "@pitwall/rpc-client";
import type { DockviewApi } from "dockview-react";
import { forwardRef, useImperativeHandle } from "react";
import { createRoot, type Root } from "react-dom/client";
import { toast } from "sonner";
import { afterEach, beforeEach, expect, vi } from "vitest";

import defaultTheme from "../../../../../extensions/theme-default/themes/default.json";
import { App } from "../../App";
import { openTab, tabParams } from "../../editor/editor-tabs";
import type { KeymapNative } from "../../keymap/native";
import { act, endTest, it } from "../../test-support/ended-test";
import { createThemeStore } from "../../theme/theme-store";
import { ThemeStoreProvider } from "../../theme/theme-switcher";
import { parseTheme } from "../../theme/tokens";
import { type DbDocumentSpec, dbDocumentKey } from "../documents/spec";
import { dbStatusStore } from "../safety/db-status";
import { SAVE_DELAY_MS } from "./console-document";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

interface Call {
  method: string;
  params: Record<string, unknown>;
}

const host = vi.hoisted(() => ({
  calls: [] as { method: string; params: Record<string, unknown> }[],
  emit: (_projectId: string, _topic: string, _payload: unknown) => {},
  send: (_m: object) => {},
  /** 콘솔 파일의 글(`project.console.read`). */
  consoleText: null as string | null,
  /** `db.session.resume` 의 답. */
  resumed: null as unknown,
  /** 저장된 프로젝트 상태(`project.state.*`). */
  state: null as unknown,
  /** 실행 스트림을 끝까지 보낸다(아니면 `accepted` · `started` 뒤에 멈춰 있다). */
  executeEnds: true,
  /** 분류의 등급 · 해시(문장 글 → 등급). */
  grade: (_text: string) => "read" as string,
  /** 실행 계획의 답. */
  plan: { format: "json", text: "[]" } as { format: string; text: string },
  /** 마지막 실행의 id(`accepted`). */
  execution: "",
  /** `db.session.unattendedRollbacks` 의 답. */
  unattended: [] as unknown[],
  /** 참이면 다음 `db.session.open` 이 `3101`(비밀 필요)로 답한다(한 번). */
  openNeedsSecret: false,
  /** 있으면 `project.console.read` 가 이 글의 오류로 답한다. */
  consoleReadError: null as string | null,
  /** 있으면 `project.console.read` · `write` · `db.session.open` 이 이것이 풀린 뒤에 답한다. */
  readGate: null as Promise<void> | null,
  writeGate: null as Promise<void> | null,
  openGate: null as Promise<void> | null,
  resumeGate: null as Promise<void> | null,
  /** `3101` 의 `data.connectionId`(Host 가 비밀을 요구한 연결). */
  secretFor: "dev",
}));

/** 손으로 푸는 약속(느린 Host). */
function gate(): { promise: Promise<void>; open(): void } {
  let open!: () => void;
  const promise = new Promise<void>((r) => (open = r));
  return { promise, open };
}

const SECRET_TARGET = vi.hoisted(() => ({
  driver: "postgres",
  host: "localhost",
  port: 5432,
  user: "app",
  tunnel: null,
  sslmode: "disable",
  sslrootcert: null,
  sslcert: null,
  sslkey: null,
}));

const glide = vi.hoisted(() => ({ rows: [] as number[] }));

vi.mock("@glideapps/glide-data-grid", () => ({
  CompactSelection: { empty: () => ({ toArray: () => [] }) },
  GridCellKind: { Text: "text", Loading: "loading" },
  DataEditor: forwardRef(function FakeEditor(props: { rows: number }, ref) {
    glide.rows.push(props.rows);
    useImperativeHandle(ref, () => ({ focus: () => {} }));
    return <div data-testid="fake-glide" data-rows={props.rows} />;
  }),
}));

/** `;` 로 나눈 문장 범위(가짜 Host 의 나누기 — 끝의 `;` 와 앞뒤 공백을 뺀다). */
function split(sql: string): { from: number; to: number }[] {
  const out: { from: number; to: number }[] = [];
  let start = 0;
  for (let i = 0; i <= sql.length; i++) {
    if (i < sql.length && sql[i] !== ";") continue;
    let from = start;
    let to = i;
    while (from < to && /\s/.test(sql[from]!)) from++;
    while (to > from && /\s/.test(sql[to - 1]!)) to--;
    if (to > from) out.push({ from, to });
    start = i + 1;
  }
  return out;
}

const RANKS = ["read", "write", "unboundedWrite", "ddl", "destructive"];

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
      host.send = send;
      const session = (over: Partial<DbSessionInfo> = {}): DbSessionInfo => ({
        sessionId: "s1",
        projectId: "p1",
        connectionId: "dev",
        mode: "auto",
        state: { kind: "idle" },
        ...over,
      });
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const params = (m.params ?? {}) as Record<string, unknown>;
        host.calls.push({ method: m.method, params });
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
          case "project.state.get":
            return reply({ state: host.state, problem: null });
          case "project.state.set":
            host.state = params.state;
            return reply(null);
          case "runtime.get":
            return reply({
              provider: "shell",
              source: { kind: "loginShell", shell: "/bin/zsh" },
              status: { kind: "ok" },
              tools: [],
            } satisfies RuntimeInfo);
          case "events.subscribe": {
            const id = next++;
            subs.set(id, params as { projectId?: string; topics: string[] });
            return reply({ subscriptionId: id });
          }
          case "events.unsubscribe":
            return reply(null);
          case "db.connection.list":
            return reply({
              connections: [
                {
                  id: "dev",
                  config: { name: "dev db", driver: "postgres", color: "db.tag.green" },
                  layers: [],
                  readOnly: false,
                  state: { kind: "ready" },
                },
              ],
              problem: null,
              stateProblem: null,
              trusted: true,
            });
          case "db.session.unattendedRollbacks":
            return reply(host.unattended);
          case "db.schema.schemas":
            return reply({ items: [{ name: "public" }, { name: "ledger" }], nextCursor: null });
          case "db.schema.databases":
            return reply({ items: [{ name: "app" }], nextCursor: null });
          case "db.session.resume":
            return void (host.resumeGate ?? Promise.resolve()).then(() => reply(host.resumed));
          case "db.secret.set":
            return reply(null);
          case "db.session.open":
            if (host.openNeedsSecret) {
              host.openNeedsSecret = false;
              return send({
                id: m.id,
                error: {
                  code: 3101,
                  message: "db connection `dev` needs its password",
                  data: {
                    connectionId: host.secretFor,
                    key: "password",
                    reason: "missing",
                    target: SECRET_TARGET,
                    previous: null,
                  },
                },
              });
            }
            return void (host.openGate ?? Promise.resolve()).then(() =>
              reply({
                session: session(),
                name: "dev db",
                readOnly: false,
                production: false,
                driver: {
                  id: "postgres",
                  dialect: "postgres",
                  capabilities: {
                    sql: true,
                    kv: false,
                    serverCursor: true,
                    cancel: true,
                    transactions: true,
                    savepoints: true,
                    readOnlyTransaction: true,
                    explain: true,
                    metadata: true,
                    schemas: true,
                    multipleDatabases: true,
                    sslModes: [],
                  },
                },
                server: { version: "17", tls: "plaintext" },
              } satisfies DbSessionOpened),
            );
          case "db.session.close":
            return reply(null);
          case "db.session.setMode":
            return reply(session({ mode: params.mode as "auto" | "manual" }));
          case "db.session.commit":
          case "db.session.rollback":
            return reply(session({ mode: "manual" }));
          case "db.sql.split":
            return reply({ statements: split(String(params.sql)) });
          case "db.sql.statementAt": {
            const ranges = split(String(params.sql));
            const offset = Number(params.offset);
            let index = ranges.findIndex((r) => offset <= r.to);
            if (index < 0) index = ranges.length - 1;
            if (index > 0 && offset < ranges[index]!.from) index--;
            return reply({
              statement: index < 0 ? null : { index, range: ranges[index] },
            });
          }
          case "db.sql.classify": {
            const sql = String(params.sql);
            const ranges = split(sql);
            const chosen = (params.statements as number[] | null) ?? ranges.map((_, i) => i);
            const statements = chosen.map((index) => {
              const range = ranges[index]!;
              return {
                index,
                range,
                grade: host.grade(sql.slice(range.from, range.to)),
                reasons: [],
              };
            });
            const grade = statements
              .map((s) => s.grade)
              .reduce((a, b) => (RANKS.indexOf(b) > RANKS.indexOf(a) ? b : a), "read");
            return reply({
              statements,
              grade,
              statementsHash: `hash-${chosen.join(",")}`,
              effects: [],
            } as DbSqlClassification);
          }
          case "db.query.execute": {
            const streamId = next++;
            reply({ streamId });
            const sql = String(params.sql);
            const ranges = split(sql);
            const chosen = (params.statements as number[] | null) ?? ranges.map((_, i) => i);
            const chunk = (data: unknown) =>
              send({ method: "stream.chunk", params: { streamId, data } });
            host.execution = `x${streamId}`;
            chunk({ kind: "accepted", executionId: host.execution, statements: chosen.length });
            for (const index of chosen) {
              chunk({ kind: "started", index, range: ranges[index], grade: "read" });
              if (!host.executeEnds) return;
              chunk({
                kind: "columns",
                index,
                columns: [{ name: "n", dbType: "int4", nullable: null }],
              });
              chunk({ kind: "rows", index, rows: [[{ int: index + 1 }]] });
              chunk({ kind: "done", index, rowsAffected: null, elapsedMs: 1, truncated: false });
            }
            return send({ method: "stream.end", params: { streamId, status: "completed" } });
          }
          case "stream.credit":
          case "stream.cancel":
            return;
          case "db.query.cancel":
            return reply(null);
          case "db.query.explain":
            return reply(host.plan);
          case "project.console.read":
            return void (host.readGate ?? Promise.resolve()).then(() =>
              host.consoleReadError
                ? send({ id: m.id, error: { code: -32602, message: host.consoleReadError } })
                : reply({ text: host.consoleText }),
            );
          case "project.console.write":
            return void (host.writeGate ?? Promise.resolve()).then(() => {
              host.consoleText = String(params.text);
              reply(null);
            });
          case "project.console.delete":
            return reply(null);
          default:
            if (m.id === undefined) return;
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

let closeGuard: (() => Promise<boolean>) | undefined;
const native = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
  closeWindow: async () => {},
  guardClose: (shouldClose: () => Promise<boolean>) => {
    closeGuard = shouldClose;
    return () => {
      if (closeGuard === shouldClose) closeGuard = undefined;
    };
  },
} satisfies KeymapNative;

let root: Root | undefined;
let dock: DockviewApi | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  vi.spyOn(console, "warn").mockImplementation(() => {});
  window.history.replaceState(null, "", "/?window=project&project=p1");
  localStorage.clear();
  dbStatusStore.setState({ tabs: {}, frontTabId: undefined });
  host.calls = [];
  host.consoleText = "SELECT 1;\r\nSELECT 2;\r\nSELECT 3;";
  host.resumed = null;
  host.state = null;
  host.executeEnds = true;
  host.grade = () => "read";
  host.plan = { format: "json", text: "[]" };
  host.execution = "";
  host.openNeedsSecret = false;
  host.consoleReadError = null;
  host.readGate = null;
  host.writeGate = null;
  host.openGate = null;
  host.resumeGate = null;
  host.secretFor = "dev";
  host.unattended = [];
  glide.rows = [];
});

afterEach(async () => {
  await endTest();
  act(() => root?.unmount());
  root = undefined;
  dock = undefined;
  closeGuard = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 30)));

async function mount() {
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

/** 창을 내리고 같은 Host 상태로 다시 연다(앱을 다시 시작). */
async function restart() {
  act(() => root?.unmount());
  document.body.innerHTML = "";
  await mount();
}

const SPEC: DbDocumentSpec = {
  kind: "console",
  tabId: "tab-console-1",
  consoleId: "c-tab-console-1",
  connectionId: "dev",
  database: null,
  schema: null,
};

async function openConsole(spec: DbDocumentSpec = SPEC) {
  await act(async () => {
    openTab(dock!, { path: dbDocumentKey(spec), db: spec, title: "console" });
  });
  await settle();
}

const methods = () => host.calls.map((c) => c.method);
const callsOf = (method: string): Call[] => host.calls.filter((c) => c.method === method);
const q = <T extends Element = HTMLElement>(selector: string) =>
  document.querySelector<T>(selector);
const button = (id: string) => q<HTMLButtonElement>(`[data-testid=${id}]`)!;
const view = () =>
  EditorView.findFromDOM(q<HTMLElement>("[data-testid=db-console-editor] .cm-editor")!)!;

async function click(el: Element) {
  await act(async () => (el as HTMLElement).click());
  await settle();
}

async function connect() {
  await click(button("db-console-connect"));
  expect(callsOf("db.session.open")).toHaveLength(1);
}

/**
 * 콘솔에서 키를 누른다(창의 키맵이 받는다). 포커스는 콘솔의 뿌리에 둔다 — happy-dom 은 포커스된 CodeMirror 의 선택을 쓸 때
 * `selectionchange` 를 곧바로 내 갱신이 겹치므로(실제 웹뷰에서는 그 이벤트가 나중에 온다) 글 영역에 포커스를 두지 않는다.
 */
async function press(init: KeyboardEventInit) {
  const target = q<HTMLElement>("[data-testid=db-console]")!;
  target.focus();
  await act(async () => {
    target.dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
    );
  });
  await settle();
}

const runKey = { code: "Enter", key: "Enter", metaKey: true };

async function sessionChanged(
  state: DbSessionInfo["state"],
  mode: DbSessionInfo["mode"] = "manual",
) {
  await act(async () =>
    host.emit("p1", "db.sessionChanged", {
      session: { sessionId: "s1", projectId: "p1", connectionId: "dev", mode, state },
      notice: null,
    }),
  );
  await settle();
}

it("runs only the statement at the caret, sending the document text (not CRLF) on ⌘↩", async () => {
  await mount();
  await openConsole();
  await connect();
  // 콘솔 글은 파일의 CRLF 그대로 읽었지만 CodeMirror 는 줄바꿈을 한 단위로 센다.
  expect(view().state.doc.toString()).toBe("SELECT 1;\nSELECT 2;\nSELECT 3;");
  await act(async () => view().dispatch({ selection: { anchor: 13 } }));
  await press(runKey);
  const [at] = callsOf("db.sql.statementAt");
  expect(at!.params).toMatchObject({ sql: "SELECT 1;\nSELECT 2;\nSELECT 3;", offset: 13 });
  const [run] = callsOf("db.query.execute");
  expect(run!.params).toMatchObject({
    sessionId: "s1",
    sql: "SELECT 1;\nSELECT 2;\nSELECT 3;",
    statements: [1],
  });
  expect(document.querySelectorAll("[data-testid=db-console-result-tab]")).toHaveLength(1);
});

it("runs every selected statement and shows one result tab each", async () => {
  await mount();
  await openConsole();
  await connect();
  await act(async () =>
    view().dispatch({ selection: { anchor: 0, head: view().state.doc.length } }),
  );
  await click(button("db-console-run"));
  expect(callsOf("db.query.execute")[0]!.params.statements).toEqual([0, 1, 2]);
  const tabs = [...document.querySelectorAll<HTMLElement>("[data-testid=db-console-result-tab]")];
  expect(tabs.map((t) => t.textContent)).toEqual(["결과 1", "결과 2", "결과 3"]);
  await click(tabs[2]!);
  const shown = document.querySelectorAll<HTMLElement>("[data-testid=db-console-result-tab]");
  expect(shown[2]!.getAttribute("aria-selected")).toBe("true");
});

it("asks before DELETE FROM t: [취소] sends nothing, [실행] carries the statementsHash it got", async () => {
  host.consoleText = "DELETE FROM t";
  host.grade = (text) => (text.startsWith("DELETE") ? "unboundedWrite" : "read");
  await mount();
  await openConsole();
  await connect();
  await press(runKey);
  const dialog = q("[data-testid=db-risk-confirm]");
  expect(dialog).not.toBeNull();
  // 처음 포커스는 [취소](M3-19).
  expect(document.activeElement?.getAttribute("data-action")).toBe("cancel");
  await click(q("[data-testid=db-risk-confirm] [data-action=cancel]")!);
  expect(q("[data-testid=db-risk-confirm]")).toBeNull();
  expect(callsOf("db.query.execute")).toEqual([]);

  await press(runKey);
  await click(q("[data-testid=db-risk-confirm] [data-action=run]")!);
  const [run] = callsOf("db.query.execute");
  expect(run!.params.confirmed).toEqual({
    grade: "unboundedWrite",
    statementsHash: "hash-0",
    name: null,
  });
});

it("rolls back with db.session.rollback in a manual transaction", async () => {
  await mount();
  await openConsole();
  await connect();
  const mode = q<HTMLSelectElement>("[data-testid=db-console-mode]")!;
  await act(async () => {
    mode.value = "manual";
    mode.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await settle();
  expect(callsOf("db.session.setMode")[0]!.params).toEqual({ sessionId: "s1", mode: "manual" });
  expect(button("db-console-rollback").disabled).toBe(true);
  await sessionChanged({ kind: "inTransaction", since: Date.now() });
  expect(button("db-console-rollback").disabled).toBe(false);
  await click(button("db-console-rollback"));
  expect(callsOf("db.session.rollback")).toEqual([
    { method: "db.session.rollback", params: { sessionId: "s1" } },
  ]);
});

it("cancels the running query with ⌘F2", async () => {
  host.executeEnds = false;
  await mount();
  await openConsole();
  await connect();
  await press(runKey);
  expect(button("db-console-cancel").disabled).toBe(false);
  await press({ code: "F2", key: "F2", metaKey: true });
  expect(callsOf("db.query.cancel").map((c) => c.params)).toEqual([
    { executionId: host.execution },
  ]);
});

const consoleTab = () => dock!.panels.find((p) => tabParams(p).db !== undefined);

it("asks [커밋] [롤백] [취소] before closing a tab with an open transaction", async () => {
  await mount();
  await openConsole();
  await connect();
  await sessionChanged({ kind: "inTransaction", since: Date.now() });

  // 탭 닫기(macOS ⌘W — 탭이 있으면 탭을 닫는다): 묻고, [취소] 면 남는다.
  await press({ code: "KeyW", key: "w", metaKey: true });
  expect(q("[data-testid=db-transaction-dialog]")).not.toBeNull();
  expect(document.activeElement?.getAttribute("data-answer")).toBe("cancel");
  await click(q("[data-testid=db-transaction-dialog] [data-answer=cancel]")!);
  expect(q("[data-testid=db-transaction-dialog]")).toBeNull();
  expect(consoleTab()).toBeDefined();
  expect(methods()).not.toContain("db.session.commit");
  expect(methods()).not.toContain("db.session.rollback");

  // [커밋] 은 커밋한 뒤 탭을 닫고 세션을 닫는다.
  await press({ code: "KeyW", key: "w", metaKey: true });
  await click(q("[data-testid=db-transaction-dialog] [data-answer=commit]")!);
  expect(callsOf("db.session.commit")).toHaveLength(1);
  expect(consoleTab()).toBeUndefined();
  expect(callsOf("db.session.close").map((c) => c.params)).toEqual([{ sessionId: "s1" }]);
});

it("asks [커밋] [롤백] [취소] before closing the window or quitting with an open transaction", async () => {
  await mount();
  await openConsole();
  await connect();
  await sessionChanged({ kind: "inTransaction", since: Date.now() });

  // 창 닫기 · 앱 종료(같은 `guardClose`): 묻고, [취소] 면 닫지 않는다.
  let answer: Promise<boolean> | undefined;
  await act(async () => {
    answer = closeGuard!();
  });
  await settle();
  expect(q("[data-testid=db-transaction-dialog]")).not.toBeNull();
  await click(q("[data-testid=db-transaction-dialog] [data-answer=cancel]")!);
  expect(await answer).toBe(false);
  expect(consoleTab()).toBeDefined();

  // [롤백] 이면 롤백하고 닫아도 된다.
  await act(async () => {
    answer = closeGuard!();
  });
  await settle();
  await click(q("[data-testid=db-transaction-dialog] [data-answer=rollback]")!);
  expect(await answer).toBe(true);
  expect(callsOf("db.session.rollback")).toHaveLength(1);
});

it("restores an open console tab after a restart without opening a session", async () => {
  await mount();
  await openConsole();
  await vi.waitFor(() =>
    expect((host.state as { main?: { tabs?: unknown[] } } | null)?.main?.tabs).toEqual([
      expect.objectContaining({ path: dbDocumentKey(SPEC), db: SPEC }),
    ]),
  );
  host.calls = [];
  await restart();
  expect(q("[data-testid=db-console-editor]")).not.toBeNull();
  expect(callsOf("db.session.resume")[0]!.params).toEqual({ projectId: "p1", tabId: SPEC.tabId });
  expect(methods()).not.toContain("db.session.open");
  expect(q("[data-notice=idle]")).not.toBeNull();
});

/** Host 가 탭에 보존한 세션(열린 트랜잭션). */
const KEPT = {
  session: {
    sessionId: "s9",
    projectId: "p1",
    connectionId: "dev",
    mode: "manual",
    state: { kind: "inTransaction", since: 1 },
  },
  name: "dev db",
  readOnly: false,
  production: false,
  driver: {
    id: "postgres",
    dialect: "postgres",
    capabilities: { sql: true, transactions: true },
  },
  server: { version: "17", tls: "plaintext" },
};

it("picks up the session Host kept for the tab after a reload, without a new connection", async () => {
  host.resumed = KEPT;
  await mount();
  await openConsole();
  expect(methods()).not.toContain("db.session.open");
  expect(q("[data-testid=db-console]")!.getAttribute("data-session")).toBe("s9");
  expect(button("db-console-rollback").disabled).toBe(false);
});

it("turns [커밋] off for a failed session and leaves [롤백] on", async () => {
  await mount();
  await openConsole();
  await connect();
  await sessionChanged({ kind: "failed", since: Date.now() });
  expect(button("db-console-commit").disabled).toBe(true);
  expect(button("db-console-rollback").disabled).toBe(false);
  expect(q("[data-notice=failed]")).not.toBeNull();
});

it("takes Korean typed through an IME and does not run while composing", async () => {
  host.consoleText = "";
  await mount();
  await openConsole();
  await connect();
  const content = q<HTMLElement>("[data-testid=db-console-editor] .cm-content")!;
  expect(content.getAttribute("contenteditable")).toBe("true");
  // 조합 중의 ⌘↩ 은 실행하지 않는다.
  await act(async () => {
    content.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
    view().dispatch({
      changes: { from: 0, insert: "SELECT '한" },
      userEvent: "input.type.compose",
    });
  });
  await press({ ...runKey, isComposing: true });
  expect(callsOf("db.query.execute")).toEqual([]);
  await act(async () => {
    view().dispatch({
      changes: { from: 0, to: view().state.doc.length, insert: "SELECT '한글'" },
      userEvent: "input.type.compose",
    });
    content.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "한글" }));
  });
  await settle();
  await press(runKey);
  expect(callsOf("db.query.execute")[0]!.params.sql).toBe("SELECT '한글'");
  // 콘솔 파일에도 그대로 간다(모아 쓰기).
  await vi.waitFor(() => expect(host.consoleText).toBe("SELECT '한글'"));
});

it("shows the query plan as an indented tree of nodes, cost and rows", async () => {
  host.consoleText = "SELECT * FROM t JOIN u ON true";
  host.plan = {
    format: "json",
    text: JSON.stringify([
      {
        Plan: {
          "Node Type": "Nested Loop",
          "Startup Cost": 0,
          "Total Cost": 12.5,
          "Plan Rows": 40,
          Plans: [
            { "Node Type": "Seq Scan", "Relation Name": "t", "Total Cost": 1.5, "Plan Rows": 4 },
            { "Node Type": "Seq Scan", "Relation Name": "u", "Total Cost": 1.5, "Plan Rows": 10 },
          ],
        },
      },
    ]),
  };
  await mount();
  await openConsole();
  await connect();
  await click(button("db-console-explain"));
  expect(callsOf("db.query.explain")[0]!.params).toMatchObject({
    sessionId: "s1",
    statements: [0],
  });
  const items = [
    ...document.querySelectorAll<HTMLElement>("[data-testid=db-console-plan] [role=treeitem]"),
  ];
  expect(
    items.map((i) => [
      i.getAttribute("aria-level"),
      i.querySelector("[data-plan-label]")!.textContent,
    ]),
  ).toEqual([
    ["1", "Nested Loop"],
    ["2", "Seq Scan on t"],
    ["2", "Seq Scan on u"],
  ]);
  expect(items[0]!.textContent).toContain("0..12.5");
  expect(items[0]!.textContent).toContain("40");
});

it("reopens the session with the chosen schema and keeps the choice in the tab", async () => {
  await mount();
  await openConsole();
  await connect();
  const schema = q<HTMLSelectElement>("select[aria-label='스키마']")!;
  await act(async () => schema.dispatchEvent(new FocusEvent("focusin", { bubbles: true })));
  await settle();
  await act(async () => {
    schema.value = "ledger";
    schema.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await settle();
  expect(callsOf("db.schema.schemas")[0]!.params).toMatchObject({
    projectId: "p1",
    connectionId: "dev",
  });
  expect(callsOf("db.session.close").map((c) => c.params)).toEqual([{ sessionId: "s1" }]);
  expect(callsOf("db.session.open")[1]!.params).toMatchObject({
    connectionId: "dev",
    schema: "ledger",
    tabId: SPEC.tabId,
  });
  expect(tabParams(consoleTab()!).db).toMatchObject({ kind: "console", schema: "ledger" });
  // 트랜잭션이 열려 있으면 고르지 못한다.
  await sessionChanged({ kind: "inTransaction", since: Date.now() });
  expect(q<HTMLSelectElement>("select[aria-label='스키마']")!.disabled).toBe(true);
  expect(q<HTMLSelectElement>("[data-testid=db-console-connection]")!.disabled).toBe(true);
});

it("asks for the password when [연결] gets 3101, saves it and opens again", async () => {
  await mount();
  await openConsole();
  host.openNeedsSecret = true;
  await click(button("db-console-connect"));
  const prompt = q<HTMLElement>("[data-testid=db-access-prompt]")!;
  expect(prompt.dataset.kind).toBe("secret");
  expect(callsOf("db.session.open")).toHaveLength(1);
  const input = q<HTMLInputElement>("[data-testid=db-access-secret]")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "pw-1");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await click(prompt.querySelector("[data-action=run]")!);
  expect(callsOf("db.secret.set").map((c) => c.params)).toEqual([
    expect.objectContaining({ connectionId: "dev", key: "password", target: SECRET_TARGET }),
  ]);
  expect(callsOf("db.session.open")).toHaveLength(2);
  expect(q("[data-testid=db-access-prompt]")).toBeNull();
  expect(button("db-console-disconnect")).toBeTruthy();
});

it("leaves the console unconnected when the password question is cancelled", async () => {
  await mount();
  await openConsole();
  host.openNeedsSecret = true;
  await click(button("db-console-connect"));
  await click(q<HTMLElement>("[data-testid=db-access-prompt] [data-action=cancel]")!);
  expect(callsOf("db.secret.set")).toEqual([]);
  expect(callsOf("db.session.open")).toHaveLength(1);
  expect(q("[data-testid=db-console-disconnect]")).toBeNull();
  expect(button("db-console-connect")).toBeTruthy();
});

/**
 * 콘솔에 글을 넣고 모아 쓰기 시간(`SAVE_DELAY_MS`)을 가짜 시간으로 넘긴다 — 실시간으로 기다리면 부하에 비례해 시험이 길어진다.
 * 디바운스 타이머가 가짜 시간 안에서 생기므로 앞당겨 터진다. 그 밖(`settle` · `vi.waitFor`)은 진짜 시간이다.
 */
async function typeThenSave(change: { from: number; insert: string }) {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  try {
    await act(async () => view().dispatch({ changes: change }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(SAVE_DELAY_MS);
    });
  } finally {
    vi.useRealTimers();
  }
}

const atEnd = (insert: string) => ({ from: view().state.doc.length, insert });

it("keeps a console file it could not read: read-only, no write when typing, and reads it again", async () => {
  host.consoleReadError = "the console is not UTF-8 text";
  await mount();
  await openConsole();
  expect(q("[data-testid=db-console-notices] [data-notice=read]")!.textContent).toContain(
    "the console is not UTF-8 text",
  );
  expect(view().state.readOnly).toBe(true);
  // 키 입력은 읽기 전용이 막는다 — 막히지 않은 바뀜(붙여 넣기 경로 등)이 와도 파일을 쓰지 않는다.
  await typeThenSave({ from: 0, insert: "x" });
  await settle();
  expect(callsOf("project.console.write")).toEqual([]);
  // [다시 읽기]: 이번엔 읽히면 고칠 수 있고, 친 글을 쓴다.
  host.consoleReadError = null;
  host.consoleText = "SELECT 9";
  await click(button("db-console-reload"));
  expect(q("[data-notice=read]")).toBeNull();
  expect(view().state.readOnly).toBe(false);
  expect(view().state.doc.toString()).toBe("SELECT 9");
  await typeThenSave(atEnd(";"));
  await vi.waitFor(() => expect(host.consoleText).toBe("SELECT 9;"));
});

it("does not delete a console file it could not read when the tab closes", async () => {
  host.consoleReadError = "the console is larger than 16 MiB";
  await mount();
  await openConsole();
  await press({ code: "KeyW", key: "w", metaKey: true });
  expect(consoleTab()).toBeUndefined();
  await settle();
  expect(methods()).not.toContain("project.console.delete");
  expect(methods()).not.toContain("project.console.write");
});

it("does not delete the console file when the tab closes before it was read", async () => {
  const slow = gate();
  host.readGate = slow.promise;
  await mount();
  await openConsole();
  await press({ code: "KeyW", key: "w", metaKey: true });
  expect(consoleTab()).toBeUndefined();
  await act(async () => slow.open());
  await settle();
  expect(callsOf("project.console.read")).toHaveLength(1);
  expect(methods()).not.toContain("project.console.delete");
  expect(methods()).not.toContain("project.console.write");
});

it("writes the console one write at a time and keeps the last text", async () => {
  host.consoleText = "";
  await mount();
  await openConsole();
  const slow = gate();
  host.writeGate = slow.promise;
  const type = async (text: string) => {
    await typeThenSave(atEnd(text));
    await settle();
  };
  await type("SELECT 1");
  expect(callsOf("project.console.write").map((c) => c.params.text)).toEqual(["SELECT 1"]);
  // 앞 쓰기가 끝나기 전에는 다음을 보내지 않고, 그사이 글은 마지막 것만 남긴다.
  await type(", 2");
  await type(", 3");
  expect(callsOf("project.console.write")).toHaveLength(1);
  host.writeGate = null;
  await act(async () => slow.open());
  await vi.waitFor(() => expect(callsOf("project.console.write")).toHaveLength(2));
  expect(callsOf("project.console.write")[1]!.params.text).toBe("SELECT 1, 2, 3");
  await vi.waitFor(() => expect(host.consoleText).toBe("SELECT 1, 2, 3"));
});

it("does not ask for a password Host asked about another connection", async () => {
  await mount();
  await openConsole();
  host.openNeedsSecret = true;
  host.secretFor = "other";
  await click(button("db-console-connect"));
  expect(q("[data-testid=db-access-prompt]")).toBeNull();
  expect(callsOf("db.session.open")).toHaveLength(1);
  expect(methods()).not.toContain("db.secret.set");
  expect(q("[data-testid=db-console-notices] [data-notice=error]")).not.toBeNull();
  expect(button("db-console-connect")).toBeTruthy();
});

it("stops connecting when the tab closes while the password is asked", async () => {
  await mount();
  await openConsole();
  host.openNeedsSecret = true;
  await click(button("db-console-connect"));
  expect(q("[data-testid=db-access-prompt]")).not.toBeNull();
  // 묻는 동안은 창의 키맵이 쉬므로 탭을 직접 닫는다.
  await act(async () => consoleTab()!.api.close());
  await settle();
  expect(consoleTab()).toBeUndefined();
  expect(q("[data-testid=db-access-prompt]")).toBeNull();
  expect(methods()).not.toContain("db.secret.set");
  expect(callsOf("db.session.open")).toHaveLength(1);
});

it("closes the kept session that comes back after the tab was closed while resuming", async () => {
  host.resumed = KEPT;
  const slow = gate();
  host.resumeGate = slow.promise;
  await mount();
  await openConsole();
  expect(callsOf("db.session.resume")).toHaveLength(1);
  await press({ code: "KeyW", key: "w", metaKey: true });
  expect(consoleTab()).toBeUndefined();
  await act(async () => slow.open());
  await settle();
  expect(callsOf("db.session.close").map((c) => c.params)).toEqual([{ sessionId: "s9" }]);
});

it("closes the session that comes back after the tab was closed while connecting", async () => {
  await mount();
  await openConsole();
  const slow = gate();
  host.openGate = slow.promise;
  await click(button("db-console-connect"));
  expect(callsOf("db.session.open")).toHaveLength(1);
  await press({ code: "KeyW", key: "w", metaKey: true });
  expect(consoleTab()).toBeUndefined();
  expect(methods()).not.toContain("db.session.close");
  await act(async () => slow.open());
  await settle();
  expect(callsOf("db.session.close").map((c) => c.params)).toEqual([{ sessionId: "s1" }]);
});

it("opens a new console for the last connected connection with ⇧⌘L", async () => {
  await mount();
  await openConsole();
  await connect();
  await act(async () => {
    document.body.dispatchEvent(
      new KeyboardEvent("keydown", {
        code: "KeyL",
        key: "l",
        metaKey: true,
        shiftKey: true,
        bubbles: true,
        cancelable: true,
      }),
    );
  });
  await settle();
  const consoles = dock!.panels.filter((p) => tabParams(p).db?.kind === "console");
  expect(consoles).toHaveLength(2);
  const fresh = tabParams(consoles[1]!).db!;
  expect(fresh).toMatchObject({ kind: "console", connectionId: "dev" });
  expect(fresh.tabId).not.toBe(SPEC.tabId);
  // 새 콘솔도 사람이 누를 때까지 연결하지 않는다.
  expect(callsOf("db.session.open")).toHaveLength(1);
});

it("tells once about transactions Host rolled back while no one was attached", async () => {
  host.unattended = [
    {
      connectionId: "dev",
      name: "dev db",
      tabId: "t1",
      since: 1_700_000_000_000,
      rolledBackAt: 1_700_000_600_000,
      keptMinutes: 10,
      rollbackError: null,
    },
  ];
  await mount();
  const toasts = () =>
    [...document.querySelectorAll("[data-sonner-toast]")].map((t) => t.textContent);
  await vi.waitFor(() =>
    expect(toasts()).toEqual([expect.stringContaining("보는 사람이 없어 롤백한 트랜잭션")]),
  );
  expect(toasts()[0]).toContain("dev db");
  expect(toasts()[0]).toContain("10분 기다린 뒤 롤백했다");
  // 토스트는 sonner 의 전역 상태라 창을 다시 열어도 남는다 — 걷은 뒤 다시 연다.
  await act(async () => void toast.dismiss());
  await settle();
  await restart();
  await vi.waitFor(() => expect(callsOf("db.session.unattendedRollbacks")).toHaveLength(2));
  await act(async () => new Promise((r) => setTimeout(r, 100)));
  expect(toasts()).toEqual([]);
});

it("leaves the session to Host when the window goes away (reload), so an open transaction can be resumed", async () => {
  await mount();
  await openConsole();
  await connect();
  await sessionChanged({ kind: "inTransaction", since: Date.now() });
  // 창이 내려가는 것(새로고침 · 웹뷰 재시작)은 탭을 닫는 것이 아니다 — 세션을 닫지 않는다(Host 가 보존한다, M3-05 ⓘ).
  await restart();
  expect(methods()).not.toContain("db.session.close");
  expect(callsOf("db.session.resume").at(-1)!.params).toEqual({
    projectId: "p1",
    tabId: SPEC.tabId,
  });
});
