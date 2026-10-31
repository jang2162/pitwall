// @vitest-environment happy-dom
// 테이블 데이터(14-m3-plan.md M3-17 의 완료 기준)를 프로젝트 창(App kind="project")에서 가짜 Host 로 본다: 수정 · 추가 · 삭제 →
// 미리보기 글이 `db.edit.preview` 의 것이고 [적용] 이 같은 `previewHash` 로 `db.edit.apply` 를 부른다, 키 없는 테이블은 편집이 꺼져
// 있고 까닭이 보인다, WHERE 칸에 `1=1; DROP TABLE t` 를 넣으면 실행 RPC 가 거절을 돌려주고 창이 그것을 보인다(거절 자체는 M3-05 ·
// M3-17 의 Rust 시험 — 가짜 Host 는 Host 와 같은 꼴로 거절한다), 미제출 변경이 있는 탭을 닫으면 묻는다(탭 닫기 · 창 닫기), 탭이
// 열린 채 다시 시작하면 연결 RPC 가 불리지 않는다. 그 밖에 헤더 클릭 정렬이 ORDER BY 를 바꿔 다시 읽기, FK ⌘+클릭이 같은 액션으로
// 참조 테이블을 그 키로 열기, 적용 실패(영향 행) 뒤 변경이 남기, 키 열 · 잘린 값은 편집기를 열지 않기, ⌘N · ⌘⌫ · ⌘↩ 키.
//
// Glide 는 canvas 라 happy-dom 에서 그릴 수 없어 넘긴 것(셀 · 콜백)을 기록하는 가짜로 바꾼다(그리드 자체는 src/db/grid 의 시험).

import type {
  ConfigView,
  DbSessionOpened,
  HostInfo,
  KeymapFile,
  Project,
  ProjectTrust,
  RuntimeInfo,
} from "@pitwall/rpc-client";
import type { DockviewApi } from "dockview-react";
import { forwardRef, useImperativeHandle } from "react";
import { createRoot, type Root } from "react-dom/client";
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
import { bundledTranslator } from "../../i18n/testing";
import { dbTableDataStrings } from "./strings";
import { markOpenedHere } from "./table-data-window";

const KO = dbTableDataStrings(bundledTranslator("ko"));

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
  state: null as unknown,
  /** `db.edit.rowKey` 의 키(없으면 키 없는 테이블). */
  key: ["id"] as string[] | null,
  /** 읽는 행(`id` · `name` · `user_id`). */
  rows: [] as unknown[][],
  /** 있으면 다음 `db.edit.apply` 가 이 오류로 답한다(한 번). */
  applyError: null as object | null,
  readOnly: false,
}));

interface FakeGlide {
  rows: number;
  getCellContent: (cell: [number, number]) => { displayData?: string; data?: string };
  onCellActivated: (cell: [number, number]) => void;
  onCellClicked?: (cell: [number, number], e: { metaKey: boolean; ctrlKey: boolean }) => void;
  onGridSelectionChange: (s: unknown) => void;
  onHeaderClicked: (
    col: number,
    e: { metaKey: boolean; ctrlKey: boolean; preventDefault: () => void },
  ) => void;
}

const glide = vi.hoisted(() => ({ props: [] as FakeGlide[] }));

vi.mock("@glideapps/glide-data-grid", () => ({
  CompactSelection: { empty: () => ({ toArray: () => [] }) },
  GridCellKind: { Text: "text", Loading: "loading" },
  DataEditor: forwardRef(function FakeEditor(props: FakeGlide, ref) {
    glide.props.push(props);
    useImperativeHandle(ref, () => ({
      focus: () => {},
      getBounds: (col: number, row: number) => ({
        x: col * 100,
        y: 60 + row * 24,
        width: 100,
        height: 24,
      }),
    }));
    return <div data-testid="fake-glide" data-rows={props.rows} />;
  }),
}));

const NOT_A_QUERY = vi.hoisted(() => ({
  code: -32003,
  message: "db.query.execute { queryOnly } runs queries only",
}));

vi.mock("../../tauri-transport", async () => {
  const { createMemoryTransportPair: pair, ERROR_CODE } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../../keymap/default-keymap");
  const { i18nState } = await import("../../i18n/testing");
  NOT_A_QUERY.code = ERROR_CODE.PERMISSION_DENIED;
  return {
    connectHost: async () => {
      const [client, server] = pair();
      const subs = new Map<number, { projectId?: string; topics: string[] }>();
      let next = 1;
      let sessions = 0;
      const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const params = (m.params ?? {}) as Record<string, unknown>;
        host.calls.push({ method: m.method, params });
        const reply = (result: unknown) => send({ id: m.id, result });
        const fail = (error: object) => send({ id: m.id, error });
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
            return reply([]);
          case "db.session.resume":
            return reply(null);
          case "db.session.open":
            sessions++;
            return reply({
              session: {
                sessionId: `s${sessions}`,
                projectId: "p1",
                connectionId: "dev",
                mode: "auto",
                state: { kind: "idle" },
              },
              name: "dev db",
              readOnly: host.readOnly,
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
            } satisfies DbSessionOpened);
          case "db.session.close":
            return reply(null);
          case "db.edit.rowKey":
            return reply({
              key: host.key && { index: "t_pkey", columns: host.key, primary: true },
            });
          case "db.schema.foreignKeys":
            return reply({
              items: [
                {
                  name: "t_user_fk",
                  columns: ["user_id"],
                  references: { database: null, schema: "public", name: "users" },
                  referencedColumns: ["id"],
                },
              ],
              nextCursor: null,
            });
          case "db.sql.insertText": {
            const value = (params.rows as unknown[][])[0]![0] as Record<string, unknown>;
            const literal = "int" in value ? String(value.int) : `'${String(value.text)}'`;
            return reply({ text: `INSERT INTO MY_TABLE ("v") VALUES (${literal});\n` });
          }
          case "db.query.execute": {
            const sql = String(params.sql);
            // Host 의 관문과 같은 꼴: 조회 문장 하나가 아니면 아무것도 돌리지 않고 거절한다.
            if (params.queryOnly === true && /;\s*\S/.test(sql))
              return fail({
                code: NOT_A_QUERY.code,
                message: NOT_A_QUERY.message,
                data: {
                  reason: "notAQuery",
                  statements: [
                    {
                      index: 1,
                      range: { from: 0, to: 1 },
                      grade: "destructive",
                      reasons: ["drop"],
                    },
                  ],
                },
              });
            const streamId = next++;
            reply({ streamId });
            const chunk = (data: unknown) =>
              send({ method: "stream.chunk", params: { streamId, data } });
            chunk({ kind: "accepted", executionId: `x${streamId}`, statements: 1 });
            chunk({ kind: "started", index: 0, range: { from: 0, to: sql.length }, grade: "read" });
            chunk({
              kind: "columns",
              index: 0,
              columns: [
                { name: "id", dbType: "int4", nullable: false },
                { name: "name", dbType: "text", nullable: true },
                { name: "user_id", dbType: "int4", nullable: true },
              ],
            });
            chunk({ kind: "rows", index: 0, rows: host.rows });
            chunk({ kind: "done", index: 0, rowsAffected: null, elapsedMs: 1, truncated: false });
            return send({ method: "stream.end", params: { streamId, status: "completed" } });
          }
          case "stream.credit":
          case "stream.cancel":
            return;
          case "db.edit.preview":
            return reply({
              statements: [
                {
                  index: 0,
                  change: { kind: "delete", index: 0 },
                  sql: 'DELETE FROM "app"."public"."t" WHERE "id" = 2',
                  grade: "write",
                },
                {
                  index: 1,
                  change: { kind: "update", index: 0 },
                  sql: `UPDATE "app"."public"."t" SET "name" = 'uno' WHERE "id" = 1`,
                  grade: "write",
                },
              ],
              grade: "write",
              key: ["id"],
              applies: "ownTransaction",
              previewHash: "ph-1",
            });
          case "db.edit.apply":
            if (host.applyError) {
              const error = host.applyError;
              host.applyError = null;
              return fail(error);
            }
            return reply({
              statements: 3,
              rowsAffected: 3,
              session: {
                sessionId: "s1",
                projectId: "p1",
                connectionId: "dev",
                mode: "auto",
                state: { kind: "idle" },
              },
            });
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
  host.state = null;
  host.key = ["id"];
  host.rows = [
    [{ int: 1 }, { text: "one" }, { int: 7 }],
    [{ int: 2 }, { text: "two" }, "null"],
  ];
  host.applyError = null;
  host.readOnly = false;
  glide.props = [];
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

async function restart() {
  act(() => root?.unmount());
  document.body.innerHTML = "";
  await mount();
}

const SPEC: DbDocumentSpec = {
  kind: "tableData",
  tabId: "tab-table-1",
  connectionId: "dev",
  object: { database: "app", schema: "public", name: "t" },
  where: "",
  orderBy: "",
};

/** 탭을 연다(되살린 탭처럼 — 연결하지 않는다). `here` 면 이 창에서 사람이 연 탭(곧바로 연결한다). */
async function openTable(spec: DbDocumentSpec = SPEC, here = false) {
  if (here) markOpenedHere(spec.tabId);
  await act(async () => {
    openTab(dock!, { path: dbDocumentKey(spec), db: spec, title: "t" });
  });
  await settle();
}

const methods = () => host.calls.map((c) => c.method);
const callsOf = (method: string): Call[] => host.calls.filter((c) => c.method === method);
const q = <T extends Element = HTMLElement>(selector: string) =>
  document.querySelector<T>(selector);
const button = (id: string) => q<HTMLButtonElement>(`[data-testid=${id}]`)!;
const grid = () => glide.props[glide.props.length - 1]!;
const tableTabs = () => dock!.panels.filter((p) => tabParams(p).db?.kind === "tableData");

async function click(el: Element) {
  await act(async () => (el as HTMLElement).click());
  await settle();
}

async function connect() {
  await click(button("db-table-data-connect"));
  expect(callsOf("db.session.open")).toHaveLength(1);
}

/** 셀을 편집한다: 더블클릭(↩) → 제자리 칸에 글 → Enter. */
async function edit(col: number, row: number, text: string) {
  await act(async () => grid().onCellActivated([col, row]));
  await settle();
  const input = q<HTMLInputElement>("[data-testid=db-table-data-inline-editor] input")!;
  expect(input).not.toBeNull();
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    setter.call(input, text);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () => {
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  });
  await settle();
}

async function typeInto(testId: string, text: string) {
  const input = q<HTMLInputElement>(`[data-testid=${testId}]`)!;
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    setter.call(input, text);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () => {
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  });
  await settle();
}

async function selectRow(row: number) {
  await act(async () =>
    grid().onGridSelectionChange({
      current: { cell: [0, row], range: { x: 0, y: row, width: 1, height: 1 }, rangeStack: [] },
      rows: { toArray: () => [] },
      columns: { toArray: () => [] },
    }),
  );
  await settle();
}

/** 테이블 데이터 안에서 키를 누른다(창의 키맵이 받는다). */
async function press(init: KeyboardEventInit) {
  const target = q<HTMLElement>("[data-testid=db-table-data]")!;
  target.focus();
  await act(async () => {
    target.dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
    );
  });
  await settle();
}

it("submits edits, an added row and a deleted row through the Host's preview, and applies with the same previewHash", async () => {
  await mount();
  await openTable();
  await connect();
  // 읽기는 조회만(`queryOnly`) — 테이블 이름은 늘 따옴표.
  expect(callsOf("db.query.execute")[0]!.params).toEqual({
    sessionId: "s1",
    sql: 'SELECT * FROM "public"."t"',
    queryOnly: true,
  });
  // 세션은 테이블의 DB 로 연다(PG).
  expect(callsOf("db.session.open")[0]!.params).toMatchObject({
    connectionId: "dev",
    database: "app",
  });
  expect(callsOf("db.edit.rowKey")[0]!.params).toEqual({ sessionId: "s1", table: SPEC.object });

  await edit(1, 0, "uno");
  await click(button("db-table-data-add-row"));
  expect(grid().rows).toBe(3);
  await edit(0, 2, "3");
  await edit(1, 2, "three");
  await selectRow(1);
  await click(button("db-table-data-delete-row"));
  expect(q("[data-testid=db-table-data-pending]")!.textContent).toContain("3");

  // 변경 대기 표시: 색 + 글자 + 접근성 글(셀 글은 그리드가 그린다).
  expect(grid().getCellContent([1, 0]).displayData).toBe("✎ uno");
  expect(grid().getCellContent([1, 0]).data).toBe("바뀜: uno");
  expect(grid().getCellContent([0, 2]).displayData).toBe("+ 3");
  expect(grid().getCellContent([1, 1]).data).toBe("지울 행: two");
  expect(grid().getCellContent([2, 0]).displayData).toBe("7");

  await click(button("db-table-data-submit"));
  const [preview] = callsOf("db.edit.preview");
  const changes = {
    updates: [
      {
        key: [{ column: "id", value: { int: 1 } }],
        values: [{ column: "name", value: { text: "uno" } }],
      },
    ],
    inserts: [
      {
        values: [
          { column: "id", value: { text: "3" } },
          { column: "name", value: { text: "three" } },
        ],
      },
    ],
    deletes: [{ key: [{ column: "id", value: { int: 2 } }] }],
  };
  expect(preview!.params).toEqual({ sessionId: "s1", table: SPEC.object, changes });
  // 미리보기의 글은 Host 가 준 그대로다 — 화면은 DML 을 만들지 않는다.
  const dialog = q("[data-testid=db-table-data-preview]")!;
  expect([...dialog.querySelectorAll("[data-statement] pre")].map((e) => e.textContent)).toEqual([
    'DELETE FROM "app"."public"."t" WHERE "id" = 2',
    `UPDATE "app"."public"."t" SET "name" = 'uno' WHERE "id" = 1`,
  ]);
  // 처음 포커스는 [취소].
  expect(document.activeElement?.getAttribute("data-answer")).toBe("cancel");
  expect(methods()).not.toContain("db.edit.apply");

  await click(dialog.querySelector("[data-answer=apply]")!);
  const [apply] = callsOf("db.edit.apply");
  expect(apply!.params).toEqual({
    sessionId: "s1",
    table: SPEC.object,
    changes,
    previewHash: "ph-1",
  });
  // 적용되면 변경을 비우고 다시 읽는다.
  expect(q("[data-testid=db-table-data-preview]")).toBeNull();
  expect(callsOf("db.query.execute")).toHaveLength(2);
  expect(q("[data-testid=db-table-data-pending]")).toBeNull();
  expect(q("[data-notice=applied]")).not.toBeNull();
});

it("does not apply when the preview is cancelled, and keeps the changes", async () => {
  await mount();
  await openTable();
  await connect();
  await edit(1, 0, "uno");
  await click(button("db-table-data-submit"));
  await click(q("[data-testid=db-table-data-preview] [data-answer=cancel]")!);
  expect(q("[data-testid=db-table-data-preview]")).toBeNull();
  expect(methods()).not.toContain("db.edit.apply");
  expect(q("[data-testid=db-table-data-pending]")).not.toBeNull();
});

it("turns editing off on a table without a key and says why", async () => {
  host.key = null;
  await mount();
  await openTable();
  await connect();
  expect(button("db-table-data-add-row").disabled).toBe(true);
  expect(button("db-table-data-delete-row").disabled).toBe(true);
  expect(button("db-table-data-submit").disabled).toBe(true);
  expect(q("[data-notice=block]")!.textContent).toBe(KO.block.noKey(""));
  await act(async () => grid().onCellActivated([1, 0]));
  await settle();
  expect(q("[data-testid=db-table-data-inline-editor]")).toBeNull();
  // ⌘N 도 행을 더하지 않는다.
  await press({ code: "KeyN", key: "n", metaKey: true });
  expect(grid().rows).toBe(2);
});

it("turns editing off on a read-only connection", async () => {
  host.readOnly = true;
  await mount();
  await openTable();
  await connect();
  expect(button("db-table-data-add-row").disabled).toBe(true);
  expect(q("[data-notice=block]")!.textContent).toBe(KO.block.readOnly(""));
});

it("shows the Host's refusal when the WHERE fragment makes more than a query", async () => {
  await mount();
  await openTable();
  await connect();
  await typeInto("db-table-data-where", "1=1; DROP TABLE t");
  const run = callsOf("db.query.execute")[1]!;
  // 화면은 조각을 검사하지 않고 그대로 보낸다 — Host 가 정본이다.
  expect(run.params).toEqual({
    sessionId: "s1",
    sql: 'SELECT * FROM "public"."t"\nWHERE 1=1; DROP TABLE t',
    queryOnly: true,
  });
  const refusal = q("[data-testid=db-refusal]")!;
  expect(refusal.dataset.reason).toBe("notAQuery");
  // 조각은 탭 params 에 남는다.
  expect(tabParams(tableTabs()[0]!).db).toMatchObject({ where: "1=1; DROP TABLE t" });
});

it("asks before closing a tab or the window with unsubmitted changes", async () => {
  await mount();
  await openTable();
  await connect();
  await edit(1, 0, "uno");

  // 탭 닫기(⌘W): 묻고, [취소] 면 남는다.
  await press({ code: "KeyW", key: "w", metaKey: true });
  const dialog = () => q("[data-testid=git-confirm-dialog]");
  expect(dialog()).not.toBeNull();
  expect(dialog()!.textContent).toContain("1");
  await click([...dialog()!.querySelectorAll("button")].find((b) => b.textContent === "취소")!);
  expect(tableTabs()).toHaveLength(1);
  expect(methods()).not.toContain("db.session.close");

  // 창 닫기 · 앱 종료(같은 `guardClose`): 묻고, [취소] 면 닫지 않는다.
  let answer: Promise<boolean> | undefined;
  await act(async () => {
    answer = closeGuard!();
  });
  await settle();
  expect(dialog()).not.toBeNull();
  await click([...dialog()!.querySelectorAll("button")].find((b) => b.textContent === "취소")!);
  expect(await answer).toBe(false);

  // [버리고 닫기] 면 탭을 닫고 세션을 닫는다.
  await press({ code: "KeyW", key: "w", metaKey: true });
  await click([...dialog()!.querySelectorAll("button")].find((b) => b.textContent !== "취소")!);
  expect(tableTabs()).toHaveLength(0);
  expect(callsOf("db.session.close").map((c) => c.params)).toEqual([{ sessionId: "s1" }]);
});

it("closes a tab without asking when nothing is pending", async () => {
  await mount();
  await openTable();
  await connect();
  await press({ code: "KeyW", key: "w", metaKey: true });
  expect(q("[data-testid=git-confirm-dialog]")).toBeNull();
  expect(tableTabs()).toHaveLength(0);
});

it("restores an open tab after a restart without calling any connection RPC", async () => {
  await mount();
  await openTable({ ...SPEC, where: "id > 0", orderBy: "name DESC" }, true);
  // 이 창에서 사람이 연 탭은 곧바로 연결했다.
  expect(callsOf("db.session.open")).toHaveLength(1);
  await vi.waitFor(() =>
    expect((host.state as { main?: { tabs?: unknown[] } } | null)?.main?.tabs).toEqual([
      expect.objectContaining({
        db: expect.objectContaining({ kind: "tableData", where: "id > 0" }),
      }),
    ]),
  );
  host.calls = [];
  await restart();
  expect(q("[data-testid=db-table-data]")).not.toBeNull();
  for (const method of [
    "db.session.open",
    "db.session.resume",
    "db.edit.rowKey",
    "db.schema.foreignKeys",
    "db.query.execute",
  ])
    expect(methods()).not.toContain(method);
  expect(q("[data-notice=idle]")).not.toBeNull();
  expect(q<HTMLInputElement>("[data-testid=db-table-data-where]")!.value).toBe("id > 0");
  // [연결] 을 누르면 그 조각으로 읽는다.
  await connect();
  expect(callsOf("db.query.execute")[0]!.params.sql).toBe(
    'SELECT * FROM "public"."t"\nWHERE id > 0\nORDER BY name DESC',
  );
});

it("sorts on header click by changing ORDER BY and reading again", async () => {
  await mount();
  await openTable();
  await connect();
  await act(async () =>
    grid().onHeaderClicked(1, { metaKey: false, ctrlKey: false, preventDefault: () => {} }),
  );
  await settle();
  expect(callsOf("db.query.execute")[1]!.params.sql).toBe(
    'SELECT * FROM "public"."t"\nORDER BY name ASC',
  );
  expect(q<HTMLInputElement>("[data-testid=db-table-data-order-by]")!.value).toBe("name ASC");
  await act(async () =>
    grid().onHeaderClicked(1, { metaKey: false, ctrlKey: false, preventDefault: () => {} }),
  );
  await settle();
  expect(callsOf("db.query.execute")[2]!.params.sql).toBe(
    'SELECT * FROM "public"."t"\nORDER BY name DESC',
  );
  // 제출하지 않은 변경이 있으면 다시 읽지 않는다.
  await edit(1, 0, "uno");
  await act(async () =>
    grid().onHeaderClicked(1, { metaKey: false, ctrlKey: false, preventDefault: () => {} }),
  );
  await settle();
  expect(callsOf("db.query.execute")).toHaveLength(3);
});

it("follows a foreign key on ⌘+click through db.tableData.open with that key", async () => {
  await mount();
  await openTable();
  await connect();
  // 보통 클릭은 아무것도 하지 않는다.
  await act(async () => grid().onCellClicked!([2, 0], { metaKey: false, ctrlKey: false }));
  await settle();
  expect(tableTabs()).toHaveLength(1);
  await act(async () => grid().onCellClicked!([2, 0], { metaKey: true, ctrlKey: false }));
  await settle();
  expect(callsOf("db.sql.insertText")[0]!.params).toMatchObject({
    dialect: "postgres",
    rows: [[{ int: 7 }]],
  });
  const tabs = tableTabs();
  expect(tabs).toHaveLength(2);
  expect(tabParams(tabs[1]!).db).toMatchObject({
    kind: "tableData",
    connectionId: "dev",
    object: { database: "app", schema: "public", name: "users" },
    where: "id = 7",
  });
  // 사람이 연 탭이라 곧바로 연결해 그 키로 읽는다.
  await vi.waitFor(() => expect(callsOf("db.session.open")).toHaveLength(2));
  await settle();
  expect(callsOf("db.query.execute").at(-1)!.params.sql).toBe(
    'SELECT * FROM "public"."users"\nWHERE id = 7',
  );
});

it("opens nothing when the foreign key value is NULL", async () => {
  await mount();
  await openTable();
  await connect();
  await act(async () => grid().onCellClicked!([2, 1], { metaKey: true, ctrlKey: false }));
  await settle();
  expect(tableTabs()).toHaveLength(1);
  expect(methods()).not.toContain("db.sql.insertText");
});

it("keeps the changes and shows why when the apply fails on the affected rows", async () => {
  await mount();
  await openTable();
  await connect();
  await edit(1, 0, "uno");
  await click(button("db-table-data-submit"));
  host.applyError = {
    code: -32009,
    message: "an edit affected other than one row",
    data: {
      reason: "rowCount",
      failure: {
        statement: 0,
        change: { kind: "update", index: 0 },
        rowsAffected: 0,
        undone: "edit",
      },
      session: null,
    },
  };
  const { ERROR_CODE } = await import("@pitwall/rpc-client");
  (host.applyError as { code: number }).code = ERROR_CODE.CONFLICT;
  await click(q("[data-testid=db-table-data-preview] [data-answer=apply]")!);
  const failure = q("[data-testid=db-table-data-preview-failure]")!;
  expect(failure.textContent).toContain("수정 1");
  expect(failure.textContent).toContain("0");
  expect(q("[data-testid=db-table-data-pending]")).not.toBeNull();
  expect(callsOf("db.query.execute")).toHaveLength(1);
});

it("does not open an editor on a key column of a read row, but does on an added row", async () => {
  await mount();
  await openTable();
  await connect();
  await act(async () => grid().onCellActivated([0, 0]));
  await settle();
  expect(q("[data-testid=db-table-data-inline-editor]")).toBeNull();
  await click(button("db-table-data-add-row"));
  await edit(0, 2, "9");
  expect(grid().getCellContent([0, 2]).displayData).toBe("+ 9");
});

it("sets NULL from the editor and drops the change when the value goes back", async () => {
  await mount();
  await openTable();
  await connect();
  await act(async () => grid().onCellActivated([1, 0]));
  await settle();
  await click(q("[data-testid=db-table-data-edit-null]")!);
  expect(grid().getCellContent([1, 0]).displayData).toBe("✎ NULL");
  await edit(1, 0, "one");
  expect(grid().getCellContent([1, 0]).displayData).toBe("one");
  expect(q("[data-testid=db-table-data-pending]")).toBeNull();
});

it("adds, deletes and submits with ⌘N, ⌘⌫ and ⌘↩, and reverts with ⌘⌥Z", async () => {
  await mount();
  await openTable();
  await connect();
  await press({ code: "KeyN", key: "n", metaKey: true });
  expect(grid().rows).toBe(3);
  await selectRow(0);
  await press({ code: "Backspace", key: "Backspace", metaKey: true });
  expect(grid().getCellContent([1, 0]).data).toBe("지울 행: one");
  await press({ code: "KeyZ", key: "z", metaKey: true, altKey: true });
  expect(grid().rows).toBe(2);
  expect(q("[data-testid=db-table-data-pending]")).toBeNull();
  await edit(1, 0, "uno");
  await press({ code: "Enter", key: "Enter", metaKey: true });
  expect(callsOf("db.edit.preview")).toHaveLength(1);
  expect(q("[data-testid=db-table-data-preview]")).not.toBeNull();
});

it("takes the table data keys with focus inside the grid area too", async () => {
  await mount();
  await openTable();
  await connect();
  // 그리드 영역(결과 화면의 뿌리 — `dbGridFocus`)에 포커스: 가장 가까운 키맵 컨텍스트가 그리드의 것이어도 산다.
  const inGrid = q<HTMLElement>("[data-testid=db-grid-copy-as-button]")!;
  inGrid.focus();
  await act(async () => {
    inGrid.dispatchEvent(
      new KeyboardEvent("keydown", {
        bubbles: true,
        cancelable: true,
        code: "KeyN",
        key: "n",
        metaKey: true,
      }),
    );
  });
  await settle();
  expect(grid().rows).toBe(3);
});

it("leaves ⌘⌫ to the WHERE field", async () => {
  await mount();
  await openTable();
  await connect();
  await selectRow(0);
  const where = q<HTMLInputElement>("[data-testid=db-table-data-where]")!;
  where.focus();
  await act(async () => {
    where.dispatchEvent(
      new KeyboardEvent("keydown", {
        bubbles: true,
        cancelable: true,
        code: "Backspace",
        key: "Backspace",
        metaKey: true,
      }),
    );
  });
  await settle();
  expect(q("[data-testid=db-table-data-pending]")).toBeNull();
});
