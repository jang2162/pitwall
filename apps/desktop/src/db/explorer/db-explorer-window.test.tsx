// @vitest-environment happy-dom
// DB 탐색기(M3-13)를 창에서 본다. Host 는 가짜 `call`(부른 메서드 · 인자를 적는다)이고 액션 레지스트리는 진짜다 — 다른 행의 액션
// (`db.console.open` 등)은 시험이 등록하거나 빼서 「등록되지 않았으면 끈다」 를 본다.
//
// 본다(14 의 M3-13 완료 기준): 미신뢰 프로젝트에서 연결 행을 눌러도 연결 RPC 가 불리지 않는다, `db.schemaChanged` 뒤 펼친 노드만
// 다시 읽는다, 프로덕션 연결 행이 색 + 라벨이다, 등록되지 않은 「새 콘솔」 이 꺼져 있다. 그 밖에 연결 상태(연결 중 · 연결됨 · 끊김 ·
// 설정 바뀜), 끊기, 「더 보기」 쪽 · 서버 쪽 찾기, 행 수 · 삭제 · 더블클릭 · 끌어 넣기, 빈 상태 · Redis 행.

import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { LocalizationProvider } from "@fluent/react";
import { ActionRegistry } from "@pitwall/keymap";
import {
  type DbConnectionEntry,
  type DbConnectionList,
  type EventMessage,
  type ObjectInfo,
  RpcCallError,
} from "@pitwall/rpc-client";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, vi } from "vitest";

import { fakeTreeLayout } from "../../file-tree/testing";
import { bundledLocalization } from "../../i18n/testing";
import { act, endTest, it } from "../../test-support/ended-test";
import { tokenCssVar } from "../../theme/tokens";
import { DbExplorerView, FILTER_DELAY_MS, type ExplorerOps } from "./db-explorer";
import { DRAG_THRESHOLD } from "./explorer-drag";
import { followExplorerEvents } from "./explorer-feature";
import { type ExplorerCalls, ExplorerModel } from "./explorer-model";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const PROJECT = "p1";

interface Call {
  method: string;
  params: Record<string, unknown>;
}

const host = {
  calls: [] as Call[],
  list: undefined as unknown as DbConnectionList,
  tables: [] as ObjectInfo[],
  views: [] as ObjectInfo[],
  /** 메서드 → 가짜 답(없으면 기본). 던지면 그 오류로 답한다. */
  override: new Map<string, (params: Record<string, unknown>) => unknown>(),
};

function entry(
  id: string,
  config: Partial<NonNullable<DbConnectionEntry["config"]>> = {},
  state: DbConnectionEntry["state"] = { kind: "ready" },
): DbConnectionEntry {
  return {
    id,
    config: {
      name: id,
      driver: "postgres",
      host: "localhost",
      database: "billing",
      ...config,
    },
    layers: [],
    readOnly: false,
    state,
  };
}

const named = (prefix: string, n: number, kind: "table" | "view"): ObjectInfo[] =>
  Array.from({ length: n }, (_, i) => ({
    name: `${prefix}${String(i).padStart(5, "0")}`,
    kind,
    comment: null,
  }));

/** Host 처럼 거르고 나눈다(커서 = 다음 번호). */
function objectPage(params: Record<string, unknown>) {
  const all = params.kind === "view" ? host.views : host.tables;
  const filter = typeof params.filter === "string" ? params.filter.toLowerCase() : undefined;
  const matching = filter ? all.filter((o) => o.name.toLowerCase().includes(filter)) : all;
  const from = typeof params.cursor === "string" ? Number(params.cursor) : 0;
  const limit = Number(params.limit);
  const items = matching.slice(from, from + limit);
  const next = from + limit;
  return { items, nextCursor: next < matching.length ? String(next) : null };
}

function answer(method: string, params: Record<string, unknown>): unknown {
  const custom = host.override.get(method);
  if (custom) return custom(params);
  switch (method) {
    case "db.connection.list":
      return structuredClone(host.list);
    case "db.schema.databases":
      return { items: [{ name: "billing" }], nextCursor: null };
    case "db.schema.schemas":
      return { items: [{ name: "public" }, { name: "sales" }], nextCursor: null };
    case "db.schema.objects":
      return objectPage(params);
    case "db.schema.columns":
      return {
        items: [
          { name: "id", dbType: "int8", nullable: false, default: null, comment: null },
          { name: "status", dbType: "text", nullable: true, default: null, comment: null },
        ],
        nextCursor: null,
      };
    case "db.schema.indexes":
      return {
        items: [{ name: "pk", columns: ["id"], primary: true, unique: true, partial: false }],
        nextCursor: null,
      };
    case "db.schema.foreignKeys":
      return { items: [], nextCursor: null };
    case "db.query.page":
      return {
        columns: [{ name: "count", dbType: "int8", nullable: false }],
        rows: [[{ int: 1204 }]],
        truncated: false,
      };
    case "db.schema.disconnect":
    case "db.schema.refresh":
    case "db.connection.delete":
      return null;
    default:
      throw new Error(`unexpected ${method}`);
  }
}

const calls: ExplorerCalls = {
  call: (async (method: string, params: Record<string, unknown>) => {
    host.calls.push({ method, params });
    await Promise.resolve();
    return answer(method, params);
  }) as unknown as ExplorerCalls["call"],
};

const called = (method: string) => host.calls.filter((c) => c.method === method);
const opens = () => host.calls.filter((c) => c.method !== "db.connection.list");

let root: Root | undefined;
let element: HTMLDivElement;
let registry: ActionRegistry;
let ops: ExplorerOps & {
  copy: ReturnType<typeof vi.fn>;
  confirm: ReturnType<typeof vi.fn>;
  notify: ReturnType<typeof vi.fn>;
  openDdl: ReturnType<typeof vi.fn>;
};
let model: ExplorerModel;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  host.calls.length = 0;
  host.override.clear();
  host.tables = named("t", 3, "table");
  host.views = named("v", 2, "view");
  host.list = {
    connections: [
      entry("pg"),
      entry("lite", { driver: "sqlite", host: null, database: null, path: "a.db" }),
    ],
    problem: null,
    stateProblem: null,
    trusted: true,
  };
  element = document.createElement("div");
  document.body.append(element);
  registry = new ActionRegistry();
  ops = {
    copy: vi.fn(async () => {}),
    confirm: vi.fn(async () => true),
    notify: vi.fn(),
    openDdl: vi.fn(),
    shortcut: () => "⌘B",
  };
  fakeTreeLayout(600);
});

afterEach(async () => {
  await endTest();
  await act(async () => root?.unmount());
  root = undefined;
  element.remove();
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

async function settle() {
  for (let i = 0; i < 5; i++) await act(async () => new Promise((r) => setTimeout(r, 0)));
}

async function mount(pageSize?: number) {
  model = new ExplorerModel({ calls, projectId: PROJECT, pageSize });
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <LocalizationProvider l10n={bundledLocalization("ko")}>
        <DbExplorerView model={model} registry={registry} ops={ops} />
      </LocalizationProvider>,
    ),
  );
  await act(async () => model.reloadList());
  await settle();
}

const tree = () => document.querySelector<HTMLElement>("[role=tree]")!;
const row = (kind: string, name: string) =>
  document.querySelector<HTMLElement>(`[data-row="${kind}"][data-name="${CSS.escape(name)}"]`);
const rowsOf = (kind: string) =>
  [...document.querySelectorAll<HTMLElement>(`[data-row="${kind}"]`)].map((r) =>
    r.getAttribute("data-name"),
  );
const statusOf = (connection: string) =>
  row("connection", connection)?.querySelector("[data-status]")?.getAttribute("data-status");

async function mouse(
  el: Element,
  type: "click" | "dblclick" | "contextmenu",
  init: MouseEventInit = {},
) {
  await act(async () => {
    el.dispatchEvent(
      new MouseEvent(type, {
        bubbles: true,
        cancelable: true,
        button: type === "contextmenu" ? 2 : 0,
        ...init,
      }),
    );
  });
  await settle();
}

async function press(init: KeyboardEventInit) {
  await act(async () => tree().focus());
  await act(async () => {
    tree().dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
    );
  });
  await settle();
}

/** 행을 펼친다(▸ 를 누른 것과 같다). */
async function expand(kind: string, name: string) {
  const r = row(kind, name);
  expect(r, `${kind} ${name}`).not.toBeNull();
  await mouse(r!.firstElementChild!, "click");
}

/** 행의 우클릭 메뉴 항목. */
async function menuItem(kind: string, name: string, item: string) {
  await mouse(row(kind, name)!, "contextmenu");
  return document.querySelector<HTMLElement>(`[data-menu="${item}"]`);
}

async function closeMenu() {
  await act(async () => {
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  });
  await settle();
}

/** pg → billing → public → 테이블. */
async function openTables() {
  await expand("connection", "pg");
  await expand("database", "billing");
  await expand("schema", "public");
  const tables = document.querySelector<HTMLElement>(`[data-row="group"][data-connection="pg"]`)!;
  await mouse(tables.firstElementChild!, "click");
}

function fakeEvents() {
  let listener: ((event: EventMessage) => void) | undefined;
  const client = {
    subscribe: vi.fn(async (_params: unknown, l: (event: EventMessage) => void) => {
      listener = l;
      return { id: 1, unsubscribe: async () => {} };
    }),
  };
  return {
    client: client as unknown as Parameters<typeof followExplorerEvents>[0],
    emit: async (topic: string, payload: unknown) => {
      await act(async () =>
        listener!({ subscriptionId: 1, projectId: PROJECT, topic, payload: payload as never }),
      );
      await settle();
    },
  };
}

it("lists an untrusted project's connections and opens nothing however a row is pressed", async () => {
  host.list = {
    connections: [entry("pg", {}, { kind: "untrusted" })],
    problem: null,
    stateProblem: null,
    trusted: false,
  };
  registry.register({ id: "db.console.open", label: "console", run: vi.fn() });
  await mount();
  expect(document.querySelector("[data-testid=db-explorer-untrusted]")?.textContent).toBe(
    "이 프로젝트를 신뢰해야 연결할 수 있다 — 지금은 연결 목록만 보인다",
  );
  expect(statusOf("pg")).toBe("untrusted");
  const r = row("connection", "pg")!;
  expect(r.getAttribute("aria-expanded")).toBeNull();
  await mouse(r, "click");
  await mouse(r, "dblclick");
  await mouse(r.firstElementChild!, "click");
  await press({ key: "ArrowRight" });
  await press({ key: "Enter" });
  expect(await menuItem("connection", "pg", "connect")).toHaveProperty("dataset.disabled", "");
  expect(document.querySelector("[data-menu=newConsole]")).toHaveProperty("dataset.disabled", "");
  await closeMenu();
  await act(async () => model.connect("pg"));
  expect(opens()).toEqual([]);
  expect(rowsOf("database")).toEqual([]);
});

it("reads only the expanded nodes again after db.schemaChanged", async () => {
  await mount();
  const events = fakeEvents();
  const stop = followExplorerEvents(events.client, PROJECT, model);
  await openTables();
  await expand("object", "t00000");
  await expand("detail", "열");
  // 뷰 · 인덱스는 펼쳤다 접는다 — 읽어 둔 것이 있지만 보이지 않는다.
  const views = () =>
    document.querySelectorAll<HTMLElement>(`[data-row="group"][data-connection="pg"]`)[1]!;
  await mouse(views().firstElementChild!, "click");
  await mouse(views().firstElementChild!, "click");
  await expand("detail", "인덱스");
  await expand("detail", "인덱스");
  // 다른 연결도 펼쳐 둔다 — 그 연결은 다시 읽지 않는다.
  await expand("connection", "lite");
  const liteTables = document.querySelector<HTMLElement>(
    `[data-row="group"][data-connection="lite"]`,
  )!;
  await mouse(liteTables.firstElementChild!, "click");
  expect(rowsOf("column")).toEqual(["id", "status"]);

  host.calls.length = 0;
  await events.emit("db.schemaChanged", { projectId: PROJECT, connectionId: "pg" });
  const reread = host.calls.map((c) => `${c.method} ${String(c.params.kind ?? "")}`.trim());
  expect(reread.sort()).toEqual(
    [
      "db.schema.columns",
      "db.schema.databases",
      "db.schema.objects table",
      "db.schema.schemas",
    ].sort(),
  );
  expect(host.calls.every((c) => c.params.connectionId === "pg")).toBe(true);
  expect(rowsOf("column")).toEqual(["id", "status"]);

  // 접혀 있던 것은 버렸다 — 다시 펼치면 새로 읽는다.
  host.calls.length = 0;
  await mouse(views().firstElementChild!, "click");
  await expand("detail", "인덱스");
  expect(host.calls.map((c) => `${c.method} ${String(c.params.kind ?? "")}`.trim()).sort()).toEqual(
    ["db.schema.indexes", "db.schema.objects view"].sort(),
  );
  stop();
});

it("marks a production connection with its color and a label, and a read-only one with a label", async () => {
  host.list.connections = [
    entry("prod", { production: true, color: "db.tag.green" }),
    entry("ro", { readOnly: true, color: "#336699" }),
  ];
  await mount();
  const prod = row("connection", "prod")!;
  const mark = prod.querySelector<HTMLElement>("[data-testid=db-connection-mark]")!;
  expect(mark.dataset.production).toBe("true");
  expect(
    prod.querySelector<HTMLElement>("[data-testid=db-connection-color]")!.style.backgroundColor,
  ).toBe(`var(${tokenCssVar("db.production")})`);
  expect(prod.querySelector("[data-label=production]")?.textContent).toBe("프로덕션");
  const ro = row("connection", "ro")!;
  expect(ro.querySelector("[data-label=readOnly]")?.textContent).toBe("읽기 전용");
  expect(ro.querySelector("[data-label=production]")).toBeNull();
  // 드라이버 아이콘은 이름을 단다.
  expect(prod.querySelector("svg[aria-label]")?.getAttribute("aria-label")).toBe("PostgreSQL");
});

it("turns New Console off until db.console.open is registered, then runs it for the row", async () => {
  await mount();
  expect(await menuItem("connection", "pg", "newConsole")).toHaveProperty("dataset.disabled", "");
  await closeMenu();
  const console = vi.fn();
  const off = registry.register({
    id: "db.console.open",
    label: "console",
    args: true,
    run: console,
  });
  const item = await menuItem("connection", "pg", "newConsole");
  expect(item?.dataset.disabled).toBeUndefined();
  await mouse(item!, "click");
  expect(console).toHaveBeenCalledWith({ connectionId: "pg" });

  await openTables();
  await mouse((await menuItem("object", "t00001", "newConsole"))!, "click");
  expect(console).toHaveBeenLastCalledWith({ connectionId: "pg", database: "billing" });
  off();
  expect(await menuItem("object", "t00001", "newConsole")).toHaveProperty("dataset.disabled", "");
  await closeMenu();
});

it("opens table data on a double-click only when db.tableData.open is registered", async () => {
  await mount();
  await openTables();
  await mouse(row("object", "t00002")!, "dblclick");
  expect(await menuItem("object", "t00002", "tableData")).toHaveProperty("dataset.disabled", "");
  await closeMenu();
  const open = vi.fn();
  registry.register({ id: "db.tableData.open", label: "data", args: true, run: open });
  await mouse(row("object", "t00002")!, "dblclick");
  expect(open).toHaveBeenCalledWith({
    connectionId: "pg",
    object: { database: "billing", schema: "public", name: "t00002" },
  });
  await press({ key: "Enter" });
  expect(open).toHaveBeenCalledTimes(2);
});

it("shows connecting, connected and disconnected, and reconnects only when expanded again", async () => {
  let release: () => void = () => {};
  host.override.set(
    "db.schema.databases",
    () =>
      new Promise((r) => (release = () => r({ items: [{ name: "billing" }], nextCursor: null }))),
  );
  await mount();
  expect(statusOf("pg")).toBe("idle");
  await expand("connection", "pg");
  expect(statusOf("pg")).toBe("connecting");
  expect(row("connection", "pg")!.textContent).toContain("연결 중");
  await act(async () => release());
  await settle();
  expect(statusOf("pg")).toBe("connected");
  expect(rowsOf("database")).toEqual(["billing"]);

  host.override.set("db.schema.schemas", () => {
    throw new RpcCallError({
      code: 3003,
      message: "db error",
      data: {
        error: { kind: "disconnected", message: "the server closed the connection", code: null },
      },
    });
  });
  await expand("database", "billing");
  expect(statusOf("pg")).toBe("lost");
  expect(row("connection", "pg")!.querySelector("[data-status]")!.getAttribute("title")).toContain(
    "the server closed the connection",
  );
  expect(document.querySelector("[data-row=error]")?.textContent).toContain(
    "the server closed the connection",
  );

  host.override.delete("db.schema.schemas");
  host.override.set("db.schema.databases", () => ({
    items: [{ name: "billing" }],
    nextCursor: null,
  }));
  await expand("connection", "pg"); // 접는다
  host.calls.length = 0;
  await expand("connection", "pg"); // 다시 펼치면 새로 읽는다
  expect(called("db.schema.databases")).toHaveLength(1);
  expect(statusOf("pg")).toBe("connected");
});

it("drops a connection whose settings changed and reads nothing until it is expanded again", async () => {
  await mount();
  const events = fakeEvents();
  followExplorerEvents(events.client, PROJECT, model);
  await openTables();
  host.calls.length = 0;
  await events.emit("config.changed", { changed: ["db/pg"] });
  expect(statusOf("pg")).toBe("configChanged");
  expect(row("connection", "pg")!.textContent).toContain("설정 바뀜 — 다시 연결");
  expect(rowsOf("database")).toEqual([]);
  expect(opens()).toEqual([]);
  expect(called("db.connection.list")).toHaveLength(1);

  // 그사이 스키마가 바뀌어도 다시 붙지 않는다.
  await events.emit("db.schemaChanged", { projectId: PROJECT, connectionId: "pg" });
  expect(opens()).toEqual([]);
  // 다른 설정(언어 등)의 config.changed 는 탐색기와 상관없다.
  await events.emit("config.changed", { changed: ["ui.language"] });
  expect(called("db.connection.list")).toHaveLength(1);

  await expand("connection", "pg");
  expect(called("db.schema.databases")).toHaveLength(1);
  expect(statusOf("pg")).toBe("connected");
});

it("disconnects through db.schema.disconnect and folds the connection", async () => {
  await mount();
  await openTables();
  expect(await menuItem("connection", "pg", "connect")).toBeNull();
  await mouse(document.querySelector("[data-menu=disconnect]")!, "click");
  expect(called("db.schema.disconnect").map((c) => c.params)).toEqual([
    { projectId: PROJECT, connectionId: "pg" },
  ]);
  expect(statusOf("pg")).toBe("idle");
  expect(rowsOf("database")).toEqual([]);
  expect(await menuItem("connection", "pg", "disconnect")).toBeNull();
  const connect = document.querySelector<HTMLElement>("[data-menu=connect]")!;
  await mouse(connect, "click");
  expect(rowsOf("database")).toEqual(["billing"]);
});

it("pages objects with Show More and filters them on the server", async () => {
  host.tables = named("t", 1200, "table");
  await mount(500);
  await openTables();
  const page = called("db.schema.objects").at(-1)!.params;
  expect(page).toMatchObject({ kind: "table", limit: 500, cursor: null, filter: null });
  // 「더 보기」 는 목록 끝이다 — 굴려 가야 그려진다.
  await press({ key: "End" });
  expect(document.querySelector("[data-row=more]")).not.toBeNull();
  await mouse(document.querySelector("[data-row=more]")!, "click");
  expect(called("db.schema.objects").at(-1)!.params).toMatchObject({ cursor: "500" });
  expect(model.rows().filter((r) => r.kind === "node" && r.node.kind === "object")).toHaveLength(
    1000,
  );

  await press({ key: "Home" });
  // 뷰 묶음은 읽어 둔 채 접어 둔다 — 찾기가 바뀌면 버린다. 1000 행 아래라 화면에 없으니 모델로 펼치고 접는다.
  const viewsId = model
    .rows()
    .find((r) => r.kind === "node" && r.node.kind === "group" && r.node.objectKind === "view")!.id;
  await act(async () => model.expand(viewsId));
  await act(async () => model.collapse(viewsId));
  const views = () =>
    document.querySelectorAll<HTMLElement>(`[data-row="group"][data-connection="pg"]`)[1]!;

  vi.useFakeTimers();
  try {
    await act(async () =>
      document.querySelector<HTMLElement>("[data-testid=db-explorer-find]")!.click(),
    );
    const input = document.querySelector<HTMLInputElement>("[data-testid=db-explorer-find-input]")!;
    const type = async (value: string) =>
      act(async () => {
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(
          input,
          value,
        );
        input.dispatchEvent(new Event("input", { bubbles: true }));
      });
    host.calls.length = 0;
    await type("t0001");
    await type("t00011");
    expect(called("db.schema.objects")).toEqual([]);
    await act(async () => vi.advanceTimersByTime(FILTER_DELAY_MS));
  } finally {
    vi.useRealTimers();
  }
  await settle();
  // 글자마다가 아니라 멈춘 뒤 한 번, 펼친 테이블 묶음만.
  expect(called("db.schema.objects").map((c) => c.params)).toEqual([
    expect.objectContaining({ kind: "table", filter: "t00011", cursor: null }),
  ]);
  expect(rowsOf("object")).toEqual(["t00011"]);
  expect(document.querySelector(`[data-row="group"][data-connection="pg"]`)!.textContent).toContain(
    "「t00011」 가 든 것만",
  );
  host.calls.length = 0;
  await mouse(views().firstElementChild!, "click");
  expect(called("db.schema.objects").map((c) => c.params)).toEqual([
    expect.objectContaining({ kind: "view", filter: "t00011" }),
  ]);
});

it("counts rows with a read-only page query and names the table as the console would", async () => {
  await mount();
  await expand("connection", "pg");
  await expand("database", "billing");
  await expand("schema", "sales");
  const tables = document.querySelector<HTMLElement>(`[data-row="group"][data-connection="pg"]`)!;
  await mouse(tables.firstElementChild!, "click");
  await mouse((await menuItem("object", "t00000", "rowCount"))!, "click");
  expect(called("db.query.page").map((c) => c.params)).toEqual([
    {
      projectId: PROJECT,
      worktree: null,
      connectionId: "pg",
      sql: "SELECT count(*) FROM sales.t00000",
      limit: 1,
    },
  ]);
  await vi.waitFor(() =>
    expect(ops.notify).toHaveBeenCalledWith({
      level: "info",
      title: "행 수",
      body: `t00000: ${(1204).toLocaleString()} 행`,
    }),
  );
});

it("cannot count rows of another PostgreSQL database than the connection's", async () => {
  host.override.set("db.schema.databases", () => ({
    items: [{ name: "billing" }, { name: "analytics" }],
    nextCursor: null,
  }));
  await mount();
  await expand("connection", "pg");
  await expand("database", "analytics");
  await expand("schema", "public");
  const tables = document.querySelector<HTMLElement>(`[data-row="group"][data-connection="pg"]`)!;
  await mouse(tables.firstElementChild!, "click");
  const item = await menuItem("object", "t00000", "rowCount");
  expect(item?.dataset.disabled).toBe("");
  expect(item?.title).toBe("연결의 기본 DB 가 아닌 DB 의 행 수는 콘솔에서 센다");
  await closeMenu();
});

it("deletes a connection only after the dangerous confirmation", async () => {
  await mount();
  ops.confirm.mockResolvedValueOnce(false);
  await mouse((await menuItem("connection", "pg", "delete"))!, "click");
  expect(called("db.connection.delete")).toEqual([]);
  await mouse((await menuItem("connection", "pg", "delete"))!, "click");
  expect(ops.confirm).toHaveBeenLastCalledWith(
    expect.objectContaining({ title: "연결 pg 을 지운다", danger: true, confirm: "삭제" }),
  );
  expect(called("db.connection.delete").map((c) => c.params)).toEqual([
    { projectId: PROJECT, worktree: null, connectionId: "pg" },
  ]);
  expect(called("db.connection.list").length).toBeGreaterThanOrEqual(2);
});

it("copies a name and opens the DDL from the menu", async () => {
  await mount();
  await openTables();
  await mouse((await menuItem("object", "t00001", "copyName"))!, "click");
  expect(ops.copy).toHaveBeenCalledWith("t00001");
  const ddl = await menuItem("object", "t00001", "ddl");
  expect(ddl?.textContent).toContain("⌘B");
  await mouse(ddl!, "click");
  expect(ops.openDdl).toHaveBeenCalledWith("pg", {
    database: "billing",
    schema: "public",
    name: "t00001",
  });
});

it("drops a table's name into an editable editor, and SELECT * FROM with ⌥", async () => {
  host.tables = [{ name: "Order Items", kind: "table", comment: null }];
  await mount();
  await openTables();
  const host_ = document.createElement("div");
  document.body.append(host_);
  const view = new EditorView({ parent: host_, state: EditorState.create({ doc: "" }) });
  const readOnly = new EditorView({
    parent: document.body.appendChild(document.createElement("div")),
    state: EditorState.create({ doc: "", extensions: [EditorState.readOnly.of(true)] }),
  });
  const drag = async (target: Element, altKey: boolean) => {
    vi.spyOn(document, "elementFromPoint").mockReturnValue(target);
    const r = row("object", "Order Items")!;
    await act(async () => {
      r.dispatchEvent(
        new PointerEvent("pointerdown", {
          bubbles: true,
          button: 0,
          pointerId: 1,
          clientX: 5,
          clientY: 5,
        }),
      );
      window.dispatchEvent(
        new PointerEvent("pointermove", {
          pointerId: 1,
          clientX: 5 + DRAG_THRESHOLD + 1,
          clientY: 5,
        }),
      );
    });
    expect(tree().dataset.dragging).toBe("");
    await act(async () => {
      window.dispatchEvent(
        new PointerEvent("pointerup", { pointerId: 1, clientX: 200, clientY: 200, altKey }),
      );
    });
    await settle();
    // happy-dom 은 포커스가 든 에디터에 고른 자리를 쓰면 selectionchange 를 그 자리에서 내 CodeMirror 의 갱신이 겹친다(브라우저는
    // 나중에 낸다) — 다음 끌기 전에 포커스를 푼다.
    await act(async () => view.contentDOM.blur());
  };
  await drag(view.contentDOM, false);
  expect(view.state.doc.toString()).toBe('"Order Items"');
  await drag(view.contentDOM, true);
  expect(view.state.doc.toString()).toBe('"Order Items"SELECT * FROM "Order Items"');
  await drag(readOnly.contentDOM, true);
  expect(readOnly.state.doc.toString()).toBe("");
  await drag(document.body, true);
  expect(tree().dataset.dragging).toBeUndefined();
  view.destroy();
  readOnly.destroy();
});

it("offers Add Connection on an empty list once the dialog's action exists", async () => {
  host.list.connections = [];
  await mount();
  const add = () =>
    document.querySelector<HTMLButtonElement>("[data-testid=db-explorer-empty-add]")!;
  expect(document.querySelector("[data-testid=db-explorer-empty]")?.textContent).toContain(
    "이 프로젝트에 DB 연결이 없다",
  );
  expect(add().disabled).toBe(true);
  expect(document.querySelector<HTMLButtonElement>("[data-testid=db-explorer-add]")!.disabled).toBe(
    true,
  );
  const run = vi.fn();
  await act(async () => void registry.register({ id: "db.connection.add", label: "add", run }));
  expect(add().disabled).toBe(false);
  await act(async () => add().click());
  expect(run).toHaveBeenCalledTimes(1);
});

it("does not expand a Redis connection and opens the Redis screen on a double-click", async () => {
  host.list.connections = [entry("cache", { driver: "redis", database: null })];
  await mount();
  const r = row("connection", "cache")!;
  expect(r.getAttribute("aria-expanded")).toBeNull();
  await mouse(r, "dblclick");
  expect(opens()).toEqual([]);
  const open = vi.fn();
  registry.register({ id: "db.redis.open", label: "redis", args: true, run: open });
  await mouse(r, "dblclick");
  expect(open).toHaveBeenCalledWith({ connectionId: "cache" });
  expect(await menuItem("connection", "cache", "newConsole")).toBeNull();
  await closeMenu();
});

it("shows a secret-needed connection and the broken approval record before asking again", async () => {
  host.list.connections = [
    entry(
      "pg",
      {},
      {
        kind: "secretRequired",
        reason: "missing",
        target: {
          driver: "postgres",
          host: "localhost",
          port: 5432,
          user: null,
          tunnel: null,
          sslmode: "disable",
          certs: null,
        } as never,
        previous: null,
      },
    ),
  ];
  host.list.stateProblem = "/data/projects/p1/db.json: expected value at line 1";
  await mount();
  expect(statusOf("pg")).toBe("secretRequired");
  expect(row("connection", "pg")!.textContent).toContain("비밀 필요");
  expect(document.querySelector("[data-testid=db-explorer-state-problem]")?.textContent).toContain(
    "/data/projects/p1/db.json",
  );
});

it("draws only the visible rows of ten thousand tables", async () => {
  host.tables = named("t", 10_000, "table");
  await mount(500);
  await openTables();
  expect(model.rows().length).toBeGreaterThan(500);
  expect(document.querySelectorAll("[data-row]").length).toBeLessThan(80);
  expect(called("db.schema.objects")).toHaveLength(1);
});
