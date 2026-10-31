// DB 탐색기의 트리 상태(docs/design/14-m3-plan.md M3-13). 화면은 db-explorer.tsx, Host 연결은 explorer-feature.tsx 다.
//
// - 트리: 연결 → DB → 스키마 → 「테이블」 · 「뷰」 → 테이블 · 뷰 → 「열」 · 「인덱스」 · 「외래 키」 → 항목. 드라이버마다 단계가
//   다르다(`TreeShape` — PG 는 DB 와 스키마, MySQL 은 DB 만, SQLite 는 둘 다 없다). Redis 연결은 펼치지 않는다(Redis 화면 M3-18).
// - 펼칠 때 읽는다(M3-10 의 `db.schema.*` 쪽 단위). 다음 쪽이 있으면 「더 보기」 행을 둔다. 객체 목록의 찾기는 서버 쪽 필터다
//   (`filter` — 펼친 객체 목록만 다시 읽고 접힌 것은 버려 다음에 펼칠 때 그 필터로 읽는다).
// - 연결 상태는 Host 에 없다 — 창이 RPC 결과로 정한다: 처음 읽는 동안 「연결 중」, 답이 오면 「연결됨」, 끊김 · 닿지 못함 ·
//   타임아웃 · 인증 · TLS 오류면 「끊김」. 설정이 바뀐 연결(`config.changed`)은 트리를 비우고 「설정 바뀜」 으로 두고 사람이 다시 펼칠
//   때까지 읽지 않는다(결정 M3-D13 — 저절로 다시 붙지 않는다). 목록의 상태(미신뢰 · 설정 오류 · 비밀 필요 · 확인 필요)는
//   `db.connection.list` 의 것이다.
// - 미신뢰 프로젝트 · 설정이 틀린 연결은 펼치지 않는다 — 연결을 여는 RPC 를 부르지 않는다.
// - `db.schemaChanged` 는 그 연결의 펼친 노드만 다시 읽고, 접힌 노드에 읽어 둔 것은 버린다(다음에 펼칠 때 새로 읽는다). 연결돼 있지
//   않은 연결이면 트리를 접고 비운다 — 다시 읽는 것이 연결을 여는 일이라서다.

import {
  type Capabilities,
  type ColumnInfo,
  type DbConnectionEntry,
  type DbConnectionList,
  type DbError,
  type DriverKind,
  ERROR_CODE,
  type ForeignKeyInfo,
  type IndexInfo,
  type ObjectRef,
  RpcCallError,
  type RpcClient,
} from "@pitwall/rpc-client";

import { DB_ERROR_CODE } from "../safety/messages";

/** 한 번에 읽는 쪽 크기(`db.schema.*` 의 상한 500). */
export const PAGE = 500;

/** 연결의 트리 모양(드라이버의 지원 플래그 `Capabilities` 에서). */
export interface TreeShape {
  /** 스키마를 볼 수 있다(`sql` 이고 `metadata`). 거짓이면 연결 행을 펼치지 않는다(Redis). */
  browse: boolean;
  /** DB 단계(`multipleDatabases`). */
  databases: boolean;
  /** 스키마 단계(`schemas`). */
  schemas: boolean;
}

/** 내장 드라이버의 모양 — 지원 플래그를 Host 에서 받기 전(또는 받지 못했을 때) 쓴다. 각 드라이버 크레이트의 `capabilities()` 와 같다. */
export const BUILTIN_SHAPES: Readonly<Record<DriverKind, TreeShape>> = {
  postgres: { browse: true, databases: true, schemas: true },
  mysql: { browse: true, databases: true, schemas: false },
  sqlite: { browse: true, databases: false, schemas: false },
  redis: { browse: false, databases: false, schemas: false },
};

/** 탐색기가 보는 객체 종류(「테이블」 · 「뷰」 묶음). */
export type BrowsedKind = "table" | "view";

/** 테이블 · 뷰 아래 묶음. */
export type DetailKind = "columns" | "indexes" | "foreignKeys";

export type ExplorerNode =
  | { kind: "connection"; id: string; connectionId: string }
  | { kind: "database"; id: string; connectionId: string; database: string }
  | { kind: "schema"; id: string; connectionId: string; database: string | null; schema: string }
  | {
      kind: "group";
      id: string;
      connectionId: string;
      database: string | null;
      schema: string | null;
      objectKind: BrowsedKind;
    }
  | {
      kind: "object";
      id: string;
      connectionId: string;
      object: ObjectRef;
      objectKind: BrowsedKind;
      comment: string | null;
    }
  | { kind: "detail"; id: string; connectionId: string; object: ObjectRef; detail: DetailKind }
  | { kind: "column"; id: string; connectionId: string; column: ColumnInfo }
  | { kind: "index"; id: string; connectionId: string; index: IndexInfo }
  | { kind: "foreignKey"; id: string; connectionId: string; foreignKey: ForeignKeyInfo };

export type ExplorerRow =
  | {
      kind: "node";
      id: string;
      node: ExplorerNode;
      depth: number;
      expanded: boolean;
      expandable: boolean;
    }
  /** 펼친 노드 아래의 상태 줄: 읽는 중 · 다음 쪽 · 오류 · 비었음. `parent` 가 그 노드다. */
  | { kind: "loading" | "more" | "empty"; id: string; parent: string; depth: number }
  | { kind: "error"; id: string; parent: string; depth: number; message: string };

/** 연결의 실행 상태(창이 RPC 결과로 정한다). */
export type ConnectionStatus =
  | { kind: "idle" }
  | { kind: "connecting" }
  | { kind: "connected" }
  | { kind: "lost"; message: string }
  | { kind: "configChanged" };

/** 연결 행의 상태 표시. 목록의 상태가 먼저다(미신뢰 · 설정 오류 · 비밀 필요 · 확인 필요), 아니면 실행 상태. */
export type ConnectionBadge =
  | "untrusted"
  | "invalid"
  | "secretRequired"
  | "approvalRequired"
  | ConnectionStatus["kind"];

/** 탐색기가 부르는 RPC(`RpcClient` 의 `call` — 시험은 가짜를 넘긴다). */
export type ExplorerCalls = Pick<RpcClient, "call">;

export interface ExplorerDeps {
  calls: ExplorerCalls;
  projectId: string;
  /** 창의 활성 워크트리(`ActiveWorktree.top`). 없으면 메인 체크아웃. */
  worktree?: string;
  /** 드라이버의 트리 모양. 없으면 `BUILTIN_SHAPES`. */
  shape?(driver: DriverKind): TreeShape | undefined;
  /** 쪽 크기(시험이 줄인다). */
  pageSize?: number;
  /**
   * 연결을 여는 RPC 가 `3101`(비밀 필요) · `3102`(확인 필요)로 답했다 — 사람에게 묻고 Host 에 넣었으면 참(그 노드를 다시 읽는다).
   * 창은 `useDbAccessPrompt`(src/db/safety/access-prompt.tsx)를 잇는다.
   */
  access?(error: unknown, connectionId: string): Promise<boolean>;
}

/** 드라이버의 지원 플래그(`db.driver.list`)에서 트리 모양. */
export function shapeOf(capabilities: Capabilities): TreeShape {
  return {
    browse: capabilities.sql && capabilities.metadata,
    databases: capabilities.multipleDatabases,
    schemas: capabilities.schemas,
  };
}

interface Kids {
  ids: string[];
  cursor: string | null;
  loading: boolean;
  error?: string;
  /** 이 읽기의 번호. 뒤에 시작한 읽기 · 버리기가 있으면 늦게 온 답을 버린다. */
  token: number;
}

const nodeId = (...parts: (string | null)[]) => JSON.stringify(parts);

/** `RpcCallError` 의 `data` 객체. */
function dataOf(error: unknown): Record<string, unknown> | undefined {
  if (!(error instanceof RpcCallError)) return undefined;
  const data = error.data;
  return data && typeof data === "object" && !Array.isArray(data)
    ? (data as Record<string, unknown>)
    : undefined;
}

/** `3003` 이면 드라이버 오류. */
export function dbErrorOf(error: unknown): DbError | undefined {
  if (!(error instanceof RpcCallError) || error.code !== DB_ERROR_CODE.DB_ERROR) return undefined;
  const inner = dataOf(error)?.error;
  return inner && typeof inner === "object" ? (inner as DbError) : undefined;
}

/** 연결이 열리지 않았거나 끊긴 오류인가(「끊김」 으로 둔다). */
function breaksConnection(error: DbError): boolean {
  return ["disconnected", "connect", "timeout", "auth", "tls"].includes(error.kind);
}

/** 사람에게 보일 오류 글. */
export function errorText(error: unknown): string {
  const db = dbErrorOf(error);
  if (db) return db.message;
  return error instanceof Error ? error.message : String(error);
}

/**
 * `run` 이 비밀 · 확인 필요(3101 · 3102)로 실패하면 `ask`(접근 묻기 — src/db/safety/access-prompt.tsx)로 묻고, 넣었으면 다시 부른다.
 * 묻지 않았거나 취소했으면 그 오류를 그대로 던진다. 다시 부를 때마다 사람의 답이 있으므로 저절로 도는 고리가 아니다.
 */
export async function withDbAccess<T>(
  run: () => Promise<T>,
  ask: (error: unknown) => Promise<boolean>,
): Promise<T> {
  for (;;) {
    try {
      return await run();
    } catch (error) {
      if (!(await ask(error))) throw error;
    }
  }
}

export class ExplorerModel {
  private readonly deps: ExplorerDeps;
  private readonly listeners = new Set<() => void>();
  private version = 0;
  private cachedRows: { version: number; rows: ExplorerRow[] } | undefined;

  private list: DbConnectionList | undefined;
  private listError: string | undefined;
  private listToken = 0;
  private readonly nodes = new Map<string, ExplorerNode>();
  private readonly kids = new Map<string, Kids>();
  private readonly expanded = new Set<string>();
  private readonly status = new Map<string, ConnectionStatus>();
  private filter = "";
  /** Host 가 알려 준 드라이버의 모양(`db.driver.list`). 없는 드라이버는 `deps.shape` · `BUILTIN_SHAPES`. */
  private shapes = new Map<string, TreeShape>();
  private selectedId: string | undefined;
  private token = 0;

  constructor(deps: ExplorerDeps) {
    this.deps = deps;
  }

  // --- 구독 ------------------------------------------------------------------------------------------------------

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  getVersion = (): number => this.version;

  private changed() {
    this.version++;
    for (const l of this.listeners) l();
  }

  // --- 목록 ------------------------------------------------------------------------------------------------------

  /** 목록(없으면 아직 읽지 않았다). */
  connectionList(): DbConnectionList | undefined {
    return this.list;
  }

  /** 목록을 읽지 못한 까닭. */
  connectionListError(): string | undefined {
    return this.listError;
  }

  entry(connectionId: string): DbConnectionEntry | undefined {
    return this.list?.connections.find((c) => c.id === connectionId);
  }

  /** 연결 목록을 (다시) 읽는다. 사라진 연결의 트리는 버린다. */
  async reloadList(): Promise<void> {
    const token = ++this.listToken;
    try {
      const list = await this.deps.calls.call("db.connection.list", this.params());
      if (token !== this.listToken) return;
      this.list = list;
      this.listError = undefined;
      const ids = new Set(list.connections.map((c) => c.id));
      const roots = [...this.nodes.values()].filter((n) => n.kind === "connection");
      for (const root of roots) {
        if (ids.has(root.connectionId)) continue;
        this.forget(root.connectionId);
        this.nodes.delete(root.id);
      }
      for (const c of list.connections) {
        const root = this.connectionNode(c.id);
        if (!this.canBrowse(c.id) && this.expanded.has(root.id)) this.forget(c.id);
      }
    } catch (e) {
      if (token !== this.listToken) return;
      this.listError = errorText(e);
    }
    this.changed();
  }

  private params() {
    return { projectId: this.deps.projectId, worktree: this.deps.worktree ?? null };
  }

  /** 드라이버마다 모양을 둔다(`db.driver.list` 의 지원 플래그). */
  setShapes(shapes: ReadonlyMap<string, TreeShape>) {
    this.shapes = new Map(shapes);
    this.changed();
  }

  /** 그 드라이버의 트리 모양. */
  shapeOf(connectionId: string): TreeShape | undefined {
    const driver = this.entry(connectionId)?.config?.driver;
    if (!driver) return undefined;
    return this.shapes.get(driver) ?? this.deps.shape?.(driver) ?? BUILTIN_SHAPES[driver];
  }

  /** 연결을 열 수 있는가 — 신뢰한 프로젝트 · 틀리지 않은 설정. 아니면 연결을 여는 항목(새 콘솔 · Redis 열기)을 끈다. */
  canOpen(connectionId: string): boolean {
    const entry = this.entry(connectionId);
    if (!entry || !this.list?.trusted) return false;
    return entry.state.kind !== "untrusted" && entry.state.kind !== "invalid" && !!entry.config;
  }

  /** 연결 행을 펼쳐 스키마를 읽을 수 있는가 — 열 수 있고 스키마가 있는 드라이버(Redis 가 아니다). */
  canBrowse(connectionId: string): boolean {
    return this.canOpen(connectionId) && this.shapeOf(connectionId)?.browse === true;
  }

  statusOf(connectionId: string): ConnectionStatus {
    return this.status.get(connectionId) ?? { kind: "idle" };
  }

  badge(connectionId: string): ConnectionBadge {
    const entry = this.entry(connectionId);
    if (!this.list?.trusted || entry?.state.kind === "untrusted") return "untrusted";
    if (!entry || entry.state.kind === "invalid") return "invalid";
    const status = this.statusOf(connectionId);
    // 열려 있는 동안의 상태가 목록의 「비밀 필요」 보다 앞선다 — 이미 연 연결은 비밀을 다시 묻지 않는다.
    if (status.kind !== "idle") return status.kind;
    if (entry.state.kind === "secretRequired") return "secretRequired";
    if (entry.state.kind === "approvalRequired") return "approvalRequired";
    return "idle";
  }

  // --- 트리 ------------------------------------------------------------------------------------------------------

  private connectionNode(connectionId: string): ExplorerNode {
    const id = nodeId("connection", connectionId);
    let node = this.nodes.get(id);
    if (!node) {
      node = { kind: "connection", id, connectionId };
      this.nodes.set(id, node);
    }
    return node;
  }

  node(id: string): ExplorerNode | undefined {
    return this.nodes.get(id);
  }

  isExpanded(id: string): boolean {
    return this.expanded.has(id);
  }

  /** 지금 걸려 있는 객체 목록 필터. */
  filterText(): string {
    return this.filter;
  }

  selected(): string | undefined {
    return this.selectedId;
  }

  select(id: string | undefined) {
    if (this.selectedId === id) return;
    this.selectedId = id;
    this.changed();
  }

  expandable(node: ExplorerNode): boolean {
    switch (node.kind) {
      case "connection":
        return this.canBrowse(node.connectionId);
      case "column":
      case "index":
      case "foreignKey":
        return false;
      default:
        return true;
    }
  }

  /** 보이는 행(펼친 노드의 자식 · 상태 줄). 바뀔 때만 다시 만든다. */
  rows(): ExplorerRow[] {
    if (this.cachedRows?.version === this.version) return this.cachedRows.rows;
    const rows: ExplorerRow[] = [];
    const walk = (id: string, depth: number) => {
      const node = this.nodes.get(id);
      if (!node) return;
      const expanded = this.expanded.has(id);
      rows.push({ kind: "node", id, node, depth, expanded, expandable: this.expandable(node) });
      if (!expanded) return;
      const kids = this.kids.get(id);
      if (!kids) return;
      for (const child of kids.ids) walk(child, depth + 1);
      const at = depth + 1;
      if (kids.loading) rows.push({ kind: "loading", id: `${id}#loading`, parent: id, depth: at });
      else if (kids.error !== undefined)
        rows.push({
          kind: "error",
          id: `${id}#error`,
          parent: id,
          depth: at,
          message: kids.error,
        });
      else if (kids.cursor !== null)
        rows.push({ kind: "more", id: `${id}#more`, parent: id, depth: at });
      else if (kids.ids.length === 0)
        rows.push({ kind: "empty", id: `${id}#empty`, parent: id, depth: at });
    };
    for (const c of this.list?.connections ?? []) walk(this.connectionNode(c.id).id, 0);
    this.cachedRows = { version: this.version, rows };
    return rows;
  }

  /** 펼친다. 연결 행이면 연결한다(설정 바뀜 · 끊김이었으면 트리를 새로 읽는다). 읽을 수 없는 연결은 아무것도 하지 않는다. */
  async expand(id: string): Promise<void> {
    const node = this.nodes.get(id);
    if (!node || !this.expandable(node)) return;
    if (node.kind === "connection") {
      const status = this.statusOf(node.connectionId).kind;
      if (status === "configChanged" || status === "lost") this.clearBelow(id);
    }
    this.expanded.add(id);
    this.changed();
    if (!this.kids.has(id) || this.kids.get(id)?.error !== undefined) await this.load(id, false);
  }

  collapse(id: string) {
    if (!this.expanded.delete(id)) return;
    this.changed();
  }

  async toggle(id: string): Promise<void> {
    if (this.expanded.has(id)) this.collapse(id);
    else await this.expand(id);
  }

  /** 「더 보기」: 다음 쪽을 잇는다. */
  async loadMore(id: string): Promise<void> {
    const kids = this.kids.get(id);
    if (!kids || kids.loading || kids.cursor === null) return;
    await this.load(id, true);
  }

  /**
   * 객체를 트리에서 찾아 고른다(Search Everywhere 의 「DB 객체」 탭 — M3-20). 연결돼 있지 않은 연결(이 창이 아직 열지 않았거나
   * 끊김 · 설정 바뀜)이면 펼치지 않고 그 연결 행만 고른다 — 연결은 사람이 누를 때만 연다(결정 M3-D13). 연결돼 있으면 DB → 스키마
   * → 「테이블」 · 「뷰」 → 객체 차례로 펼치며 내려간다: 이미 읽은 자식은 다시 읽지 않고, 아직 읽지 않은 조상은 사람이 펼친 것과
   * 같은 조회로 읽는다. 찾지 못하면(다음 쪽 · 걸린 필터 · 그새 사라짐) 거기까지 내려간 노드를 고른다. 고른 노드의 id.
   */
  async reveal(connectionId: string, object: ObjectRef, kind: BrowsedKind): Promise<string> {
    let at = this.connectionNode(connectionId);
    if (!this.canBrowse(connectionId) || this.statusOf(connectionId).kind !== "connected") {
      this.select(at.id);
      return at.id;
    }
    const matches = (node: ExplorerNode | undefined): boolean => {
      switch (node?.kind) {
        case "database":
          return node.database === object.database;
        case "schema":
          return node.schema === object.schema;
        case "group":
          return node.objectKind === kind;
        case "object":
          return node.objectKind === kind && node.object.name === object.name;
        default:
          return false;
      }
    };
    while (at.kind !== "object") {
      await this.expand(at.id);
      const next = (this.kids.get(at.id)?.ids ?? []).map((id) => this.nodes.get(id)).find(matches);
      if (!next) break;
      at = next;
    }
    this.select(at.id);
    return at.id;
  }

  /** 우클릭 「연결」: 연결 행을 펼쳐 읽는다(이미 연결돼 있으면 펼치기만). */
  async connect(connectionId: string): Promise<void> {
    await this.expand(this.connectionNode(connectionId).id);
  }

  /** 우클릭 「끊기」: 메타데이터용 연결을 닫고(`db.schema.disconnect`) 트리를 접고 비운다. */
  async disconnect(connectionId: string): Promise<void> {
    this.forget(connectionId);
    this.changed();
    await this.deps.calls.call("db.schema.disconnect", {
      projectId: this.deps.projectId,
      connectionId,
    });
  }

  /**
   * 툴바 「새로 고침」: 목록을 다시 읽고, 열어 둔 연결마다 스키마 캐시를 버린다(`db.schema.refresh` — Host 가 `db.schemaChanged` 를
   * 내면 펼친 노드를 다시 읽는다). 연결을 열지 않는다.
   */
  async refresh(): Promise<void> {
    await this.reloadList();
    const open = [...this.status].filter(([, s]) => s.kind === "connected").map(([id]) => id);
    await Promise.all(
      open.map((connectionId) =>
        this.deps.calls.call("db.schema.refresh", { projectId: this.deps.projectId, connectionId }),
      ),
    );
  }

  /**
   * 「행 수」: `SELECT count(*)` 를 쪽 단위 조회(`db.query.page` — 읽기 전용 트랜잭션)로 돌려 그 값을 글로. 비밀 · 확인이 필요하면
   * 묻고 넣었으면 다시 센다.
   */
  async countRows(connectionId: string, sql: string): Promise<string> {
    const page = await withDbAccess(
      () =>
        this.deps.calls.call("db.query.page", {
          projectId: this.deps.projectId,
          worktree: this.deps.worktree ?? null,
          connectionId,
          sql,
          limit: 1,
        }),
      async (error) => !!this.deps.access && (await this.deps.access(error, connectionId)),
    );
    const value = page.rows[0]?.[0];
    if (value && typeof value === "object") {
      if ("int" in value && value.int !== undefined)
        return typeof value.int === "number" ? value.int.toLocaleString() : value.int;
      if ("decimal" in value && value.decimal !== undefined) return value.decimal;
      if ("text" in value && value.text !== undefined) return value.text;
    }
    return "?";
  }

  /** 연결을 지운다(설정 파일 · 비밀 · 확인 기록). 열어 둔 메타데이터용 연결을 먼저 닫는다. */
  async deleteConnection(connectionId: string): Promise<void> {
    if (this.statusOf(connectionId).kind !== "idle") await this.disconnect(connectionId);
    await this.deps.calls.call("db.connection.delete", { ...this.params(), connectionId });
    await this.reloadList();
  }

  /** 객체 목록 필터를 바꾼다(서버 쪽). 펼친 객체 목록은 그 필터로 다시 읽고, 접힌 것에 읽어 둔 것은 버린다. */
  async setFilter(text: string): Promise<void> {
    if (text === this.filter) return;
    this.filter = text;
    const groups = [...this.nodes.values()].filter((n) => n.kind === "group");
    const reloads: Promise<void>[] = [];
    for (const group of groups) {
      if (!this.kids.has(group.id)) continue;
      if (this.expanded.has(group.id)) reloads.push(this.load(group.id, false));
      else this.dropKids(group.id);
    }
    this.changed();
    await Promise.all(reloads);
  }

  /** `db.schemaChanged`: 그 연결의 펼친 노드만 다시 읽는다. */
  async schemaChanged(connectionId: string): Promise<void> {
    const root = this.connectionNode(connectionId).id;
    if (!this.kids.has(root)) return;
    if (this.statusOf(connectionId).kind !== "connected") {
      this.forget(connectionId);
      this.changed();
      return;
    }
    const reloads: Promise<void>[] = [];
    for (const [id] of this.kids) {
      const node = this.nodes.get(id);
      if (node?.connectionId !== connectionId || !readsHost(node, this.shapeOf(connectionId)))
        continue;
      if (this.expanded.has(id) && this.visible(id)) reloads.push(this.load(id, false));
      else this.dropKids(id);
    }
    this.changed();
    await Promise.all(reloads);
  }

  /** `config.changed`(`db/<연결 id>`): 열었던 연결은 트리를 비우고 「설정 바뀜」 으로 둔다. 목록은 부른 쪽이 다시 읽는다. */
  configChanged(connectionIds: readonly string[]) {
    let any = false;
    for (const connectionId of connectionIds) {
      const status = this.statusOf(connectionId).kind;
      if (status === "idle") continue;
      this.forget(connectionId);
      this.status.set(connectionId, { kind: "configChanged" });
      any = true;
    }
    if (any) this.changed();
  }

  /** 노드가 보이는가(조상이 모두 펼쳐져 있다). */
  private visible(id: string): boolean {
    for (const row of this.rows()) if (row.id === id) return true;
    return false;
  }

  /** 연결의 트리와 상태를 버린다(접는다). */
  private forget(connectionId: string) {
    const root = this.connectionNode(connectionId).id;
    this.clearBelow(root);
    this.expanded.delete(root);
    this.status.delete(connectionId);
  }

  /** 노드 아래에 읽어 둔 것을 모두 버린다(늦게 온 답도 버린다). */
  private clearBelow(id: string) {
    const kids = this.kids.get(id);
    if (!kids) return;
    for (const child of kids.ids) {
      this.clearBelow(child);
      this.expanded.delete(child);
      this.nodes.delete(child);
    }
    this.kids.delete(id);
  }

  /** 접힌 노드에 읽어 둔 것을 버린다. 그 아래 펼친 것도 함께 잊는다. */
  private dropKids(id: string) {
    this.clearBelow(id);
  }

  /** 노드의 자식을 읽는다(`append` 면 다음 쪽을 잇는다). */
  private async load(id: string, append: boolean): Promise<void> {
    const node = this.nodes.get(id);
    if (!node) return;
    const shape = this.shapeOf(node.connectionId);
    if (!shape) return;
    const sync = syncChildren(node, shape);
    if (sync) {
      this.setKids(id, sync, null);
      this.changed();
      return;
    }
    const token = ++this.token;
    const before = this.kids.get(id);
    const cursor = append ? (before?.cursor ?? null) : null;
    this.kids.set(id, {
      // 다시 읽는 동안 앞의 자식을 그대로 보인다(깜박이지 않게).
      ids: before?.ids ?? [],
      cursor: before?.cursor ?? null,
      loading: true,
      token,
    });
    const status = this.statusOf(node.connectionId).kind;
    if (status !== "connected") this.status.set(node.connectionId, { kind: "connecting" });
    this.changed();
    try {
      const page = await this.fetch(node, shape, cursor);
      if (this.kids.get(id)?.token !== token) return;
      this.status.set(node.connectionId, { kind: "connected" });
      const children = page.items;
      const ids = append ? [...(before?.ids ?? [])] : [];
      if (!append) this.clearBelowKeeping(id, children);
      for (const child of children) {
        this.nodes.set(child.id, child);
        ids.push(child.id);
      }
      this.kids.set(id, { ids, cursor: page.cursor, loading: false, token });
    } catch (e) {
      if (this.kids.get(id)?.token !== token) return;
      this.failed(node, id, e);
      this.changed();
      await this.askAccess(node.connectionId, id, token, e);
      return;
    }
    this.changed();
  }

  /** 비밀 · 확인이 필요하다는 답이면 사람에게 묻고, 넣었으면 그 노드를 다시 읽는다. */
  private async askAccess(connectionId: string, id: string, token: number, error: unknown) {
    const code = error instanceof RpcCallError ? error.code : undefined;
    if (code !== DB_ERROR_CODE.SECRET_REQUIRED && code !== DB_ERROR_CODE.APPROVAL_REQUIRED) return;
    if (!this.deps.access || !(await this.deps.access(error, connectionId))) return;
    // 그사이 다른 읽기 · 버리기가 있었으면 그쪽을 따른다.
    if (this.kids.get(id)?.token !== token) return;
    await Promise.all([this.load(id, false), this.reloadList()]);
  }

  /** 다시 읽은 자식 가운데 이름이 같은 것은 펼친 상태 · 읽어 둔 것을 잇고, 사라진 것은 버린다. */
  private clearBelowKeeping(id: string, next: readonly ExplorerNode[]) {
    const kids = this.kids.get(id);
    if (!kids) return;
    const keep = new Set(next.map((n) => n.id));
    for (const child of kids.ids) {
      if (keep.has(child)) continue;
      this.clearBelow(child);
      this.expanded.delete(child);
      this.nodes.delete(child);
    }
  }

  private setKids(id: string, children: readonly ExplorerNode[], cursor: string | null) {
    const old = this.kids.get(id);
    if (old) this.clearBelowKeeping(id, children);
    for (const child of children) if (!this.nodes.has(child.id)) this.nodes.set(child.id, child);
    this.kids.set(id, {
      ids: children.map((c) => c.id),
      cursor,
      loading: false,
      token: ++this.token,
    });
  }

  private failed(node: ExplorerNode, id: string, e: unknown) {
    const kids = this.kids.get(id);
    this.kids.set(id, {
      ids: kids?.ids ?? [],
      cursor: kids?.cursor ?? null,
      loading: false,
      error: errorText(e),
      token: kids?.token ?? 0,
    });
    const db = dbErrorOf(e);
    const connectionId = node.connectionId;
    if (db && breaksConnection(db)) {
      this.status.set(connectionId, { kind: "lost", message: db.message });
      return;
    }
    if (db) {
      // DB 가 답했다(권한 · 문법 · 지원하지 않음) — 연결은 열려 있다.
      this.status.set(connectionId, { kind: "connected" });
      return;
    }
    if (this.statusOf(connectionId).kind === "connecting")
      this.status.set(connectionId, { kind: "idle" });
    // 신뢰 · 비밀 · 확인 · 설정이 바뀌었다 — 목록의 상태를 다시 읽는다.
    const code = e instanceof RpcCallError ? e.code : undefined;
    if (
      code === ERROR_CODE.PERMISSION_DENIED ||
      code === DB_ERROR_CODE.SECRET_REQUIRED ||
      code === DB_ERROR_CODE.APPROVAL_REQUIRED ||
      code === ERROR_CODE.CONFLICT
    )
      void this.reloadList();
  }

  /** Host 에서 자식 한 쪽을 읽는다. */
  private async fetch(
    node: ExplorerNode,
    shape: TreeShape,
    cursor: string | null,
  ): Promise<{ items: ExplorerNode[]; cursor: string | null }> {
    const { projectId } = this.deps;
    const worktree = this.deps.worktree ?? null;
    const connectionId = node.connectionId;
    const limit = this.deps.pageSize ?? PAGE;
    const base = { projectId, worktree, connectionId, cursor, limit };
    switch (node.kind) {
      case "connection": {
        if (shape.databases) {
          const page = await this.deps.calls.call("db.schema.databases", base);
          return {
            items: page.items.map((d) => ({
              kind: "database",
              id: nodeId("database", connectionId, d.name),
              connectionId,
              database: d.name,
            })),
            cursor: page.nextCursor,
          };
        }
        return this.schemas(node, null, cursor);
      }
      case "database":
        return this.schemas(node, node.database, cursor);
      case "group": {
        const filter = this.filter === "" ? null : this.filter;
        const page = await this.deps.calls.call("db.schema.objects", {
          ...base,
          database: node.database,
          schema: node.schema,
          kind: node.objectKind,
          filter,
        });
        return {
          items: page.items
            .filter((o) => o.kind === node.objectKind)
            .map((o) => {
              const object = { database: node.database, schema: node.schema, name: o.name };
              return {
                kind: "object",
                id: nodeId("object", connectionId, node.database, node.schema, o.kind, o.name),
                connectionId,
                object,
                objectKind: node.objectKind,
                comment: o.comment,
              };
            }),
          cursor: page.nextCursor,
        };
      }
      case "detail":
        return this.details(node, cursor);
      default:
        return { items: [], cursor: null };
    }
  }

  /** 테이블 · 뷰 아래 묶음의 항목 한 쪽. */
  private async details(
    node: Extract<ExplorerNode, { kind: "detail" }>,
    cursor: string | null,
  ): Promise<{ items: ExplorerNode[]; cursor: string | null }> {
    const connectionId = node.connectionId;
    const params = {
      projectId: this.deps.projectId,
      worktree: this.deps.worktree ?? null,
      connectionId,
      object: node.object,
      cursor,
      limit: this.deps.pageSize ?? PAGE,
    };
    const { database, schema, name } = node.object;
    const key = (item: string) => nodeId(node.detail, connectionId, database, schema, name, item);
    switch (node.detail) {
      case "columns": {
        const page = await this.deps.calls.call("db.schema.columns", params);
        return {
          items: page.items.map((column) => ({
            kind: "column",
            id: key(column.name),
            connectionId,
            column,
          })),
          cursor: page.nextCursor,
        };
      }
      case "indexes": {
        const page = await this.deps.calls.call("db.schema.indexes", params);
        return {
          items: page.items.map((index) => ({
            kind: "index",
            id: key(index.name),
            connectionId,
            index,
          })),
          cursor: page.nextCursor,
        };
      }
      case "foreignKeys": {
        const page = await this.deps.calls.call("db.schema.foreignKeys", params);
        return {
          items: page.items.map((foreignKey) => ({
            kind: "foreignKey",
            id: key(foreignKey.name),
            connectionId,
            foreignKey,
          })),
          cursor: page.nextCursor,
        };
      }
    }
  }

  /** 연결 · DB 아래: 스키마 단계가 있으면 스키마 목록, 없으면 「테이블」 · 「뷰」 묶음. */
  private async schemas(
    node: ExplorerNode,
    database: string | null,
    cursor: string | null,
  ): Promise<{ items: ExplorerNode[]; cursor: string | null }> {
    const connectionId = node.connectionId;
    const shape = this.shapeOf(connectionId);
    if (!shape?.schemas) return { items: groups(connectionId, database, null), cursor: null };
    const page = await this.deps.calls.call("db.schema.schemas", {
      projectId: this.deps.projectId,
      worktree: this.deps.worktree ?? null,
      connectionId,
      database,
      cursor,
      limit: this.deps.pageSize ?? PAGE,
    });
    return {
      items: page.items.map((s) => ({
        kind: "schema",
        id: nodeId("schema", connectionId, database, s.name),
        connectionId,
        database,
        schema: s.name,
      })),
      cursor: page.nextCursor,
    };
  }
}

function groups(
  connectionId: string,
  database: string | null,
  schema: string | null,
): ExplorerNode[] {
  return (["table", "view"] as const).map((objectKind) => ({
    kind: "group",
    id: nodeId("group", connectionId, database, schema, objectKind),
    connectionId,
    database,
    schema,
    objectKind,
  }));
}

/** Host 를 부르지 않고 정하는 자식(묶음). 아니면 `undefined`. */
function syncChildren(node: ExplorerNode, shape: TreeShape): ExplorerNode[] | undefined {
  switch (node.kind) {
    case "connection":
      return shape.databases || shape.schemas ? undefined : groups(node.connectionId, null, null);
    case "database":
      return shape.schemas ? undefined : groups(node.connectionId, node.database, null);
    case "schema":
      return groups(node.connectionId, node.database, node.schema);
    case "object": {
      const details: DetailKind[] =
        node.objectKind === "table" ? ["columns", "indexes", "foreignKeys"] : ["columns"];
      return details.map((detail) => ({
        kind: "detail",
        id: nodeId(
          "detail",
          node.connectionId,
          node.object.database,
          node.object.schema,
          node.object.name,
          node.objectKind,
          detail,
        ),
        connectionId: node.connectionId,
        object: node.object,
        detail,
      }));
    }
    default:
      return undefined;
  }
}

/** 자식을 Host 에서 읽는 노드인가(`db.schemaChanged` 에 다시 읽을 것). */
function readsHost(node: ExplorerNode, shape: TreeShape | undefined): boolean {
  return shape !== undefined && syncChildren(node, shape) === undefined;
}
