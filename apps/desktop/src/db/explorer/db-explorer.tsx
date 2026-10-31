// DB 탐색기 화면(14-m3-plan.md M3-13, 10-ui.md 「주요 툴 윈도우 상세」 의 DB 그림 · 「DB」 마우스 표). 상태는 explorer-model.ts,
// Host · 창 연결은 explorer-feature.tsx.
//
// - 툴바: + 연결 추가(`db.connection.add`) · ⟳ 새로 고침 · ⌕ 찾기(테이블 · 뷰 이름 — 서버 쪽 필터).
// - 행은 TanStack Virtual 로 보이는 것만 그린다(테이블 1만 개 스키마도 DOM 은 수십 행).
// - 연결 행: 드라이버 아이콘 · 연결 표지(색 + 「프로덕션」 · 「읽기 전용」 라벨 — M3-19 `ConnectionMark`) · 상태 글자.
// - 키보드: ↑↓ 이동, → 펼침 · 첫 자식, ← 접음 · 부모, Home/End, PageUp/PageDown, Enter 는 더블클릭과 같다.
// - 더블클릭: 테이블 · 뷰는 테이블 데이터(`db.tableData.open`), Redis 연결은 Redis 화면(`db.redis.open`), 「더 보기」 는 다음 쪽, 그 밖은
//   펼치고 접는다.
// - 우클릭: 새 콘솔 · DDL 보기(`⌘B`) · 행 수 · 이름 복사 · 테이블 데이터 열기, 연결 행은 연결 편집 · 삭제(확인) · 연결 · 끊기. 다른 행의
//   액션이 등록되지 않았으면 그 항목을 끈다.
// - 테이블 · 뷰를 끌어 SQL 콘솔에 놓으면 이름을, `⌥` 면 `SELECT * FROM …` 을 넣는다(explorer-drag.ts).

import type { ActionRegistry } from "@pitwall/keymap";
import type { DriverKind, ObjectRef } from "@pitwall/rpc-client";
import * as ContextMenu from "@radix-ui/react-context-menu";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  Boxes,
  ChevronDown,
  ChevronRight,
  Columns3,
  Database,
  Eye,
  FileBox,
  Folder,
  FolderOpen,
  FolderTree,
  KeyRound,
  Layers,
  Link2,
  ListTree,
  type LucideIcon,
  Plus,
  RefreshCw,
  Rows3,
  Search,
  Server,
  Table2,
  X,
} from "lucide-react";
import {
  type KeyboardEvent,
  type PointerEvent,
  type ReactNode,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";

import type { GitConfirm } from "../../git/dialogs/git-dialogs";
import type { NotificationInput } from "../../notifications/notifications";
import { isComposingKey } from "../../ui/ime";
import { ConnectionMark } from "../safety/connection-mark";
import { EXPLORER_ACTIONS, EXPLORER_CONTEXT, EXTERNAL_ACTIONS } from "./explorer-actions";
import { trackExplorerDrag } from "./explorer-drag";
import {
  type ConnectionBadge,
  errorText,
  type ExplorerModel,
  type ExplorerNode,
  type ExplorerRow,
} from "./explorer-model";
import { countSql, dropText } from "./sql-names";
import { type DbExplorerStrings, useDbExplorerStrings } from "./strings";

export const ROW_HEIGHT = 22;
const INDENT = 14;
/** 찾기 칸을 친 뒤 서버에 묻기까지 기다리는 시간(ms). */
export const FILTER_DELAY_MS = 200;

/** 화면이 창에서 받는 것(시험은 가짜를 넘긴다). */
export interface ExplorerOps {
  copy(text: string): Promise<void>;
  confirm(question: GitConfirm): Promise<boolean>;
  notify(input: NotificationInput): void;
  /** 그 테이블 · 뷰의 DDL 문서를 연다. */
  openDdl(connectionId: string, object: ObjectRef): void;
  /** 액션의 단축키 글(메뉴 오른쪽). */
  shortcut?(action: string): string | undefined;
}

const DRIVER_ICONS: Record<DriverKind, LucideIcon> = {
  postgres: Database,
  mysql: Server,
  sqlite: FileBox,
  redis: Boxes,
};

const DRIVER_NAMES: Record<DriverKind, string> = {
  postgres: "PostgreSQL",
  mysql: "MySQL/MariaDB",
  sqlite: "SQLite",
  redis: "Redis",
};

/** 상태 글자의 색(색만으로 가르지 않는다 — 글자가 함께 있다). */
const BADGE_TONE: Record<ConnectionBadge, string> = {
  idle: "text-ui-muted-foreground",
  connecting: "text-ui-muted-foreground",
  connected: "text-ui-foreground",
  lost: "text-ui-accent",
  configChanged: "text-ui-accent",
  secretRequired: "text-ui-accent",
  approvalRequired: "text-ui-accent",
  untrusted: "text-ui-muted-foreground",
  invalid: "text-ui-accent",
};

const rowDomId = (index: number) => `db-explorer-row-${index}`;

/** 노드의 이름(이름 복사 · 끌어 넣기). 이름이 없는 묶음은 `undefined`. */
export function nodeName(node: ExplorerNode, model: ExplorerModel): string | undefined {
  switch (node.kind) {
    case "connection":
      return model.entry(node.connectionId)?.config?.name ?? node.connectionId;
    case "database":
      return node.database;
    case "schema":
      return node.schema;
    case "object":
      return node.object.name;
    case "column":
      return node.column.name;
    case "index":
      return node.index.name;
    case "foreignKey":
      return node.foreignKey.name;
    default:
      return undefined;
  }
}

function useModel(model: ExplorerModel) {
  useSyncExternalStore(model.subscribe, model.getVersion);
}

function useRegistry(registry: ActionRegistry) {
  useSyncExternalStore(registry.subscribe, registry.getVersion);
}

function Label({
  node,
  model,
  T,
}: {
  node: ExplorerNode;
  model: ExplorerModel;
  T: DbExplorerStrings;
}) {
  const muted = "truncate text-ui-muted-foreground";
  switch (node.kind) {
    case "connection": {
      const entry = model.entry(node.connectionId);
      const config = entry?.config;
      const badge = model.badge(node.connectionId);
      const status = model.statusOf(node.connectionId);
      const hint =
        status.kind === "lost"
          ? T.lostHint(status.message)
          : status.kind === "configChanged"
            ? T.configChangedHint
            : entry?.state.kind === "invalid"
              ? entry.state.message
              : undefined;
      return (
        <>
          <ConnectionMark
            name={config?.name ?? node.connectionId}
            production={config?.production}
            readOnly={config?.readOnly}
            color={config?.color}
          />
          <span
            data-testid="db-explorer-status"
            data-status={badge}
            title={hint}
            className={`ml-1 flex-none text-[11px] ${BADGE_TONE[badge]}`}
          >
            {T.status[badge]}
          </span>
        </>
      );
    }
    case "database":
      return <span className="truncate">{node.database}</span>;
    case "schema":
      return <span className="truncate">{node.schema}</span>;
    case "group": {
      const filter = model.filterText();
      return (
        <>
          <span className="truncate">{T.groups[node.objectKind]}</span>
          {filter !== "" && <span className={`${muted} text-[11px]`}>{T.filtered(filter)}</span>}
        </>
      );
    }
    case "object":
      return (
        <span className="truncate" title={node.comment ?? undefined}>
          {node.object.name}
        </span>
      );
    case "detail":
      return <span className="truncate">{T.details[node.detail]}</span>;
    case "column": {
      const c = node.column;
      return (
        <>
          <span className="truncate">{c.name}</span>
          <span className={muted}>
            {c.dbType}
            {c.nullable ? "" : ` · ${T.item.notNull}`}
          </span>
        </>
      );
    }
    case "index": {
      const i = node.index;
      const tags = [
        i.primary ? T.item.primaryKey : undefined,
        i.unique && !i.primary ? T.item.unique : undefined,
        i.partial ? T.item.partial : undefined,
      ].filter(Boolean);
      return (
        <>
          <span className="truncate">{i.name}</span>
          <span className={muted}>
            ({i.columns.join(", ")}){tags.length > 0 ? ` · ${tags.join(" · ")}` : ""}
          </span>
        </>
      );
    }
    case "foreignKey": {
      const f = node.foreignKey;
      const target = [f.references.schema, f.references.name].filter(Boolean).join(".");
      return (
        <>
          <span className="truncate">{f.name}</span>
          <span className={muted}>
            ({f.columns.join(", ")}) → {target}({f.referencedColumns.join(", ")})
          </span>
        </>
      );
    }
  }
}

function iconOf(node: ExplorerNode, expanded: boolean, model: ExplorerModel): LucideIcon {
  switch (node.kind) {
    case "connection": {
      const driver = model.entry(node.connectionId)?.config?.driver;
      return driver ? DRIVER_ICONS[driver] : Database;
    }
    case "database":
      return Layers;
    case "schema":
      return FolderTree;
    case "group":
      return expanded ? FolderOpen : Folder;
    case "object":
      return node.objectKind === "view" ? Eye : Table2;
    case "detail":
      return node.detail === "columns" ? Columns3 : node.detail === "indexes" ? ListTree : Link2;
    case "column":
      return Rows3;
    case "index":
      return node.index.primary ? KeyRound : ListTree;
    case "foreignKey":
      return Link2;
  }
}

function RowView({
  row,
  index,
  start,
  selected,
  model,
  T,
  onPointerDown,
  onClick,
  onDoubleClick,
  onToggle,
  onContextMenu,
}: {
  row: ExplorerRow;
  index: number;
  start: number;
  selected: boolean;
  model: ExplorerModel;
  T: DbExplorerStrings;
  onPointerDown(row: ExplorerRow, e: PointerEvent<HTMLElement>): void;
  onClick(row: ExplorerRow): void;
  onDoubleClick(row: ExplorerRow): void;
  onToggle(row: ExplorerRow): void;
  onContextMenu(row: ExplorerRow): void;
}) {
  const common = {
    id: rowDomId(index),
    role: "treeitem" as const,
    "aria-level": row.depth + 1,
    "aria-selected": selected,
    style: {
      height: ROW_HEIGHT,
      transform: `translateY(${start}px)`,
      paddingLeft: row.depth * INDENT + 4,
    },
    className: `absolute top-0 left-0 flex w-full cursor-default items-center gap-1 pr-2 whitespace-nowrap select-none ${
      selected ? "bg-ui-active" : "hover:bg-ui-hover"
    }`,
    onClick: () => onClick(row),
    onDoubleClick: () => onDoubleClick(row),
    onContextMenu: () => onContextMenu(row),
  };
  if (row.kind !== "node") {
    const text =
      row.kind === "error"
        ? T.failed(row.message)
        : row.kind === "loading"
          ? T.loading
          : row.kind === "more"
            ? T.more
            : T.none;
    return (
      <div {...common} data-row={row.kind} role={row.kind === "error" ? "alert" : "treeitem"}>
        <span className="w-4 flex-none" />
        <span
          className={`truncate text-[12px] ${
            row.kind === "more"
              ? "text-ui-accent underline-offset-2 hover:underline"
              : row.kind === "error"
                ? "text-ui-accent"
                : "text-ui-muted-foreground italic"
          }`}
        >
          {text}
        </span>
      </div>
    );
  }
  const { node, expanded, expandable } = row;
  const Icon = iconOf(node, expanded, model);
  const name =
    nodeName(node, model) ??
    (node.kind === "group"
      ? T.groups[node.objectKind]
      : node.kind === "detail"
        ? T.details[node.detail]
        : undefined);
  return (
    <div
      {...common}
      aria-expanded={expandable ? expanded : undefined}
      data-row={node.kind}
      data-name={name}
      data-connection={node.connectionId}
      data-expandable={expandable ? "" : undefined}
      onPointerDown={(e) => onPointerDown(row, e)}
    >
      <span
        className="flex w-4 flex-none items-center justify-center"
        onClick={(e) => {
          if (!expandable) return;
          e.stopPropagation();
          onToggle(row);
        }}
      >
        {expandable &&
          (expanded ? (
            <ChevronDown size={12} aria-hidden />
          ) : (
            <ChevronRight size={12} aria-hidden />
          ))}
      </span>
      <Icon
        size={14}
        aria-label={
          node.kind === "connection"
            ? (() => {
                const driver = model.entry(node.connectionId)?.config?.driver;
                return driver ? DRIVER_NAMES[driver] : undefined;
              })()
            : undefined
        }
        aria-hidden={node.kind === "connection" ? undefined : true}
        className="flex-none"
      />
      <span className="flex min-w-0 items-center gap-1">
        <Label node={node} model={model} T={T} />
      </span>
    </div>
  );
}

function MenuItem({
  onSelect,
  disabled,
  title,
  shortcut,
  testId,
  children,
}: {
  onSelect(): void;
  disabled?: boolean;
  title?: string;
  shortcut?: string;
  testId: string;
  children: ReactNode;
}) {
  return (
    <ContextMenu.Item
      onSelect={onSelect}
      disabled={disabled}
      title={title}
      data-menu={testId}
      className="flex h-[24px] cursor-default items-center gap-3 px-3 outline-none data-[disabled]:text-ui-muted-foreground data-[highlighted]:bg-ui-active"
    >
      <span className="flex-1">{children}</span>
      {shortcut && <span className="text-[11px] text-ui-muted-foreground">{shortcut}</span>}
    </ContextMenu.Item>
  );
}

const SEPARATOR = <ContextMenu.Separator className="my-1 h-px bg-ui-border" />;

export interface DbExplorerViewProps {
  model: ExplorerModel;
  registry: ActionRegistry;
  ops: ExplorerOps;
  /** 오를 때마다 고른 행을 화면 안으로 굴리고 트리에 포커스를 준다. */
  focusRequest?: number;
}

export function DbExplorerView({ model, registry, ops, focusRequest = 0 }: DbExplorerViewProps) {
  const T = useDbExplorerStrings();
  useModel(model);
  useRegistry(registry);
  const rows = model.rows();
  const selectedId = model.selected();
  const selectedIndex = useMemo(
    () => (selectedId === undefined ? -1 : rows.findIndex((r) => r.id === selectedId)),
    [rows, selectedId],
  );
  const scrollRef = useRef<HTMLDivElement>(null);
  const [menuRow, setMenuRow] = useState<ExplorerRow>();
  const [finding, setFinding] = useState(model.filterText() !== "");
  const [findText, setFindText] = useState(model.filterText());
  const [dragging, setDragging] = useState(false);
  const stopDrag = useRef<() => void>(undefined);
  useEffect(() => () => stopDrag.current?.(), []);
  const [scrollPending, setScrollPending] = useState(0);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    getItemKey: (index) => rows[index]?.id ?? index,
    overscan: 12,
  });

  const scrolled = useRef(0);
  useLayoutEffect(() => {
    if (scrollPending === scrolled.current || selectedIndex < 0) return;
    scrolled.current = scrollPending;
    virtualizer.scrollToIndex(selectedIndex, { align: "auto" });
  }, [scrollPending, selectedIndex, virtualizer]);

  useEffect(() => {
    if (focusRequest === 0) return;
    setScrollPending((n) => n + 1);
    scrollRef.current?.focus({ preventScroll: true });
  }, [focusRequest]);

  // 찾기 칸: 멈춘 뒤 서버에 묻는다(글자마다 묻지 않는다).
  useEffect(() => {
    const text = findText.trim();
    if (text === model.filterText()) return;
    const timer = setTimeout(() => void model.setFilter(text), FILTER_DELAY_MS);
    return () => clearTimeout(timer);
  }, [findText, model]);

  const has = (id: string) => registry.isAvailable(id);
  const run = (id: string, args?: unknown) => void registry.run(id, args);
  const list = model.connectionList();

  const selectIndex = (index: number) => {
    const row = rows[Math.max(0, Math.min(rows.length - 1, index))];
    if (!row) return;
    model.select(row.id);
    setScrollPending((n) => n + 1);
  };

  const driverOf = (connectionId: string) => model.entry(connectionId)?.config?.driver;

  /** 더블클릭 · Enter. */
  const activate = (row: ExplorerRow) => {
    if (row.kind === "more") {
      void model.loadMore(row.parent);
      return;
    }
    if (row.kind !== "node") return;
    const { node } = row;
    if (node.kind === "object") {
      if (has(EXTERNAL_ACTIONS.tableData))
        run(EXTERNAL_ACTIONS.tableData, { connectionId: node.connectionId, object: node.object });
      return;
    }
    if (node.kind === "connection" && driverOf(node.connectionId) === "redis") {
      if (model.canOpen(node.connectionId) && has(EXTERNAL_ACTIONS.redis))
        run(EXTERNAL_ACTIONS.redis, { connectionId: node.connectionId });
      return;
    }
    if (row.expandable) void model.toggle(row.id);
  };

  const onTreeKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.target !== scrollRef.current || isComposingKey(e.nativeEvent)) return;
    const row = rows[selectedIndex];
    const page = Math.max(1, Math.floor((scrollRef.current?.clientHeight ?? 0) / ROW_HEIGHT) - 1);
    let handled = true;
    switch (e.key) {
      case "ArrowDown":
        selectIndex(selectedIndex + 1);
        break;
      case "ArrowUp":
        selectIndex(selectedIndex < 0 ? 0 : selectedIndex - 1);
        break;
      case "Home":
        selectIndex(0);
        break;
      case "End":
        selectIndex(rows.length - 1);
        break;
      case "PageDown":
        selectIndex(selectedIndex + page);
        break;
      case "PageUp":
        selectIndex(selectedIndex - page);
        break;
      case "ArrowRight":
        if (row?.kind !== "node" || !row.expandable) break;
        if (!row.expanded) void model.expand(row.id);
        else if (rows[selectedIndex + 1]?.depth === row.depth + 1) selectIndex(selectedIndex + 1);
        break;
      case "ArrowLeft": {
        if (!row) break;
        if (row.kind === "node" && row.expanded) {
          model.collapse(row.id);
          break;
        }
        for (let i = selectedIndex - 1; i >= 0; i--)
          if (rows[i]!.depth < row.depth) {
            selectIndex(i);
            break;
          }
        break;
      }
      case "Enter":
        if (row) activate(row);
        break;
      default:
        handled = false;
    }
    if (handled) {
      e.preventDefault();
      e.stopPropagation();
    }
  };

  /** 테이블 · 뷰 행을 누르면 끌기를 따라가기 시작한다(왼쪽 단추). ⌃클릭은 macOS 의 우클릭이다. */
  const startDrag = (row: ExplorerRow, e: PointerEvent<HTMLElement>) => {
    if (row.kind !== "node" || row.node.kind !== "object" || e.button !== 0 || e.ctrlKey) return;
    const { node } = row;
    const driver = driverOf(node.connectionId);
    if (!driver) return;
    stopDrag.current?.();
    stopDrag.current = trackExplorerDrag({
      start: { x: e.clientX, y: e.clientY },
      pointerId: e.pointerId,
      text: (select) => dropText(driver, node.object, select),
      onDragging: setDragging,
    });
  };

  const fail = (action: string) => (e: unknown) =>
    ops.notify({ level: "error", title: T.actionFailed(action, errorText(e)) });

  const copyName = (node: ExplorerNode) => {
    const name = nodeName(node, model);
    if (name === undefined) return;
    ops
      .copy(name)
      .then(() => ops.notify({ level: "info", title: T.copied(name) }), fail(T.menu.copyName));
  };

  const countRows = (node: Extract<ExplorerNode, { kind: "object" }>) => {
    const driver = driverOf(node.connectionId);
    if (!driver) return;
    model.countRows(node.connectionId, countSql(driver, node.object)).then(
      (count) =>
        ops.notify({
          level: "info",
          title: T.rowCount.title,
          body: T.rowCount.body(node.object.name, count),
        }),
      fail(T.menu.rowCount),
    );
  };

  const deleteConnection = async (connectionId: string) => {
    const name = model.entry(connectionId)?.config?.name ?? connectionId;
    const yes = await ops.confirm({
      title: T.deleteConfirm.title(name),
      body: T.deleteConfirm.body,
      confirm: T.deleteConfirm.confirm,
      danger: true,
    });
    if (yes) await model.deleteConnection(connectionId).catch(fail(T.menu.delete));
  };

  const target = menuRow?.kind === "node" ? menuRow.node : undefined;
  const menu = target ? menuFor(target) : null;

  function menuFor(node: ExplorerNode): ReactNode {
    const id = node.connectionId;
    const open = model.canOpen(id);
    const consoleArgs =
      node.kind === "database" || node.kind === "schema"
        ? { connectionId: id, database: node.database }
        : node.kind === "object"
          ? { connectionId: id, database: node.object.database }
          : { connectionId: id };
    const isRedis = driverOf(id) === "redis";
    const newConsole = !isRedis && (
      <MenuItem
        testId="newConsole"
        disabled={!open || !has(EXTERNAL_ACTIONS.console)}
        onSelect={() => run(EXTERNAL_ACTIONS.console, consoleArgs)}
      >
        {T.menu.newConsole}
      </MenuItem>
    );
    const copy = nodeName(node, model) !== undefined && (
      <MenuItem testId="copyName" onSelect={() => copyName(node)}>
        {T.menu.copyName}
      </MenuItem>
    );
    if (node.kind === "connection") {
      const status = model.statusOf(id).kind;
      const connected = status === "connected" || status === "connecting";
      return (
        <>
          {isRedis ? (
            <MenuItem
              testId="openRedis"
              disabled={!open || !has(EXTERNAL_ACTIONS.redis)}
              onSelect={() => run(EXTERNAL_ACTIONS.redis, { connectionId: id })}
            >
              {T.menu.openRedis}
            </MenuItem>
          ) : (
            <>
              {newConsole}
              {connected ? (
                <MenuItem
                  testId="disconnect"
                  onSelect={() => void model.disconnect(id).catch(fail(T.menu.disconnect))}
                >
                  {T.menu.disconnect}
                </MenuItem>
              ) : (
                <MenuItem
                  testId="connect"
                  disabled={!model.canBrowse(id)}
                  onSelect={() => void model.connect(id)}
                >
                  {T.menu.connect}
                </MenuItem>
              )}
            </>
          )}
          {SEPARATOR}
          <MenuItem
            testId="edit"
            disabled={!has(EXTERNAL_ACTIONS.editConnection)}
            onSelect={() => run(EXTERNAL_ACTIONS.editConnection, { connectionId: id })}
          >
            {T.menu.edit}
          </MenuItem>
          <MenuItem testId="delete" onSelect={() => void deleteConnection(id)}>
            {T.menu.delete}
          </MenuItem>
          {SEPARATOR}
          {copy}
        </>
      );
    }
    if (node.kind === "object") {
      const config = model.entry(id)?.config;
      // PG 의 쪽 단위 조회는 연결 설정의 DB 로만 간다 — 다른 DB 의 테이블은 셀 수 없다.
      const otherDatabase =
        config?.driver === "postgres" &&
        node.object.database !== null &&
        node.object.database !== (config.database ?? null);
      return (
        <>
          {newConsole}
          <MenuItem
            testId="ddl"
            shortcut={ops.shortcut?.(EXPLORER_ACTIONS.ddl)}
            onSelect={() => ops.openDdl(id, node.object)}
          >
            {T.menu.ddl}
          </MenuItem>
          <MenuItem
            testId="rowCount"
            disabled={otherDatabase}
            title={otherDatabase ? T.menu.rowCountOtherDatabase : undefined}
            onSelect={() => countRows(node)}
          >
            {T.menu.rowCount}
          </MenuItem>
          {copy}
          {SEPARATOR}
          <MenuItem
            testId="tableData"
            disabled={!has(EXTERNAL_ACTIONS.tableData)}
            onSelect={() =>
              run(EXTERNAL_ACTIONS.tableData, { connectionId: id, object: node.object })
            }
          >
            {T.menu.tableData}
          </MenuItem>
        </>
      );
    }
    if (node.kind === "database" || node.kind === "schema")
      return (
        <>
          {newConsole}
          {copy}
        </>
      );
    return copy || null;
  }

  const body = (() => {
    if (!list) {
      const error = model.connectionListError();
      return (
        <p role={error ? "alert" : "status"} className="p-2 text-ui-muted-foreground">
          {error ? T.listFailed(error) : T.loading}
        </p>
      );
    }
    if (list.connections.length === 0 && !list.problem)
      return (
        <div
          data-testid="db-explorer-empty"
          className="flex flex-col items-center gap-2 p-4 text-center text-ui-muted-foreground"
        >
          <p>{T.empty}</p>
          <button
            type="button"
            data-testid="db-explorer-empty-add"
            disabled={!has(EXTERNAL_ACTIONS.addConnection)}
            onClick={() => run(EXTERNAL_ACTIONS.addConnection)}
            className="rounded-sm border border-ui-border px-2 py-0.5 text-ui-foreground hover:bg-ui-hover disabled:opacity-40"
          >
            {T.toolbar.add}
          </button>
        </div>
      );
    return (
      <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
        {virtualizer.getVirtualItems().map((item) => {
          const row = rows[item.index];
          if (!row) return null;
          return (
            <RowView
              key={item.key}
              row={row}
              index={item.index}
              start={item.start}
              selected={item.index === selectedIndex}
              model={model}
              T={T}
              onPointerDown={startDrag}
              onClick={(r) => {
                model.select(r.id);
                if (r.kind === "more") void model.loadMore(r.parent);
              }}
              onDoubleClick={activate}
              onToggle={(r) => void model.toggle(r.id)}
              onContextMenu={(r) => {
                model.select(r.id);
                setMenuRow(r);
              }}
            />
          );
        })}
      </div>
    );
  })();

  const button =
    "flex size-5 items-center justify-center rounded-sm text-ui-muted-foreground hover:bg-ui-hover hover:text-ui-foreground disabled:opacity-40";

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex flex-none items-center gap-1 border-b border-ui-border px-2 py-0.5">
        <button
          type="button"
          aria-label={T.toolbar.add}
          title={T.toolbar.add}
          data-testid="db-explorer-add"
          className={button}
          disabled={!has(EXTERNAL_ACTIONS.addConnection)}
          onClick={() => run(EXTERNAL_ACTIONS.addConnection)}
        >
          <Plus size={14} aria-hidden />
        </button>
        <button
          type="button"
          aria-label={T.toolbar.refresh}
          title={T.toolbar.refresh}
          data-testid="db-explorer-refresh"
          className={button}
          onClick={() => void model.refresh().catch(fail(T.toolbar.refresh))}
        >
          <RefreshCw size={14} aria-hidden />
        </button>
        <button
          type="button"
          aria-label={T.toolbar.find}
          title={T.toolbar.find}
          aria-pressed={finding}
          data-testid="db-explorer-find"
          className={button}
          onClick={() => setFinding((f) => !f)}
        >
          <Search size={14} aria-hidden />
        </button>
      </div>
      {finding && (
        <div className="flex flex-none items-center gap-1 border-b border-ui-border px-2 py-0.5">
          <input
            aria-label={T.toolbar.find}
            placeholder={T.toolbar.findPlaceholder}
            data-testid="db-explorer-find-input"
            autoFocus
            value={findText}
            onChange={(e) => setFindText(e.target.value)}
            onKeyDown={(e) => {
              if (isComposingKey(e.nativeEvent)) return;
              if (e.key === "Escape") {
                e.preventDefault();
                setFindText("");
                setFinding(false);
                scrollRef.current?.focus({ preventScroll: true });
              }
            }}
            className="min-w-0 flex-1 bg-transparent text-[12px] outline-none"
          />
          {findText !== "" && (
            <button
              type="button"
              aria-label={T.toolbar.findClear}
              className={button}
              onClick={() => setFindText("")}
            >
              <X size={12} aria-hidden />
            </button>
          )}
        </div>
      )}
      {list && !list.trusted && (
        <p
          data-testid="db-explorer-untrusted"
          role="status"
          className="flex-none border-b border-ui-border px-2 py-1 text-[12px] text-ui-muted-foreground"
        >
          {T.untrusted}
        </p>
      )}
      {list?.problem && (
        <p
          role="alert"
          className="flex-none border-b border-ui-border px-2 py-1 text-[12px] text-ui-accent"
        >
          {T.configProblem(list.problem)}
        </p>
      )}
      {list?.stateProblem && (
        <p
          data-testid="db-explorer-state-problem"
          role="alert"
          className="flex-none border-b border-ui-border px-2 py-1 text-[12px] text-ui-accent"
        >
          {T.stateProblem(list.stateProblem)}
        </p>
      )}
      <ContextMenu.Root>
        <ContextMenu.Trigger asChild>
          <div
            ref={scrollRef}
            role="tree"
            aria-label={T.label}
            aria-activedescendant={selectedIndex >= 0 ? rowDomId(selectedIndex) : undefined}
            tabIndex={0}
            data-keymap-context={EXPLORER_CONTEXT}
            data-dragging={dragging ? "" : undefined}
            onKeyDown={onTreeKey}
            onContextMenu={(e) => {
              if (e.target === scrollRef.current) setMenuRow(undefined);
            }}
            className="min-h-0 flex-1 overflow-auto text-[13px] outline-none data-[dragging]:cursor-grabbing data-[dragging]:[&_*]:cursor-grabbing"
          >
            {body}
          </div>
        </ContextMenu.Trigger>
        {menu && (
          <ContextMenu.Portal>
            <ContextMenu.Content
              data-testid="db-explorer-menu"
              onCloseAutoFocus={(e) => {
                e.preventDefault();
                scrollRef.current?.focus({ preventScroll: true });
              }}
              className="min-w-[200px] rounded-md border border-ui-border bg-ui-panel py-1 text-ui-foreground shadow-lg"
            >
              {menu}
            </ContextMenu.Content>
          </ContextMenu.Portal>
        )}
      </ContextMenu.Root>
    </div>
  );
}
