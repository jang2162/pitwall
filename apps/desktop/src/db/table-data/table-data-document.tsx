// 테이블 데이터 문서(14-m3-plan.md M3-17, 10-ui.md 「에디터 영역 문서 타입」): 위 막대(연결 표지 · [연결] · WHERE · ORDER BY ·
// [다시 읽기] · 행 추가 · 행 삭제 · [제출] · [되돌리기] · 변경 수), 편집이 꺼졌으면 그 까닭, 그리드(M3-16 `ResultView`).
//
// 세션 · 읽기 · 편집의 순서는 table-data-store.ts 머리 주석. 여기는 그 상태를 그리고 창에 잇는다:
// - **셀 편집**: 더블클릭 · ↩(그리드의 `onActivate`)이 제자리 입력 칸을 그 셀 위에 연다(Enter 적용 · Esc 취소 · 칸을 떠나면 적용).
//   JSON · 여러 줄 · 긴 글(`LONG_TEXT` 자 넘음)은 오른쪽 상세 패널의 여러 줄 편집기로 연다. 두 편집기 모두 [NULL] 이 있다. 키 열
//   (읽은 행) · 잘린 값 · 지울 행은 열지 않고 까닭을 알린다.
// - **변경 대기 표시**: 고친 셀 · 추가 행 · 지울 행을 색 + 표지 글자 + 접근성 글로(그리드의 `cellMark`, 10-ui.md §6).
// - **FK 따라가기**: FK 열의 값을 ⌘(Win/Linux Ctrl)+클릭하면 액션 `db.tableData.open` 으로 참조 테이블을 그 키의 WHERE 로 연다.
// - 상태바 `db` 항목(M3-19)에 이 탭의 세션을 둔다. 연결 표지 색이 문서 테두리에 든다(프로덕션은 빨강 2px).
// - 닫기 전에 묻기: 작업 공간의 `guardClose` 에 등록만 한다(M3-15 의 틀) — 묻는 것은 창의 확인(table-data-feature.tsx).
// - **복원된 탭은 연결하지 않은 채 열린다**(결정 M3-D13) — 마운트만으로는 연결 · 세션 RPC 를 부르지 않는다. 이 창에서 사람이 연
//   탭만 곧바로 연결한다(`takeOpenedHere`).

import type { DbSessionChanged, DbValue } from "@pitwall/rpc-client";
import { Minus, Plus } from "lucide-react";
import {
  type KeyboardEvent as ReactKeyboardEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { useStore } from "zustand";

import { useEditorWorkspace } from "../../editor/editor-workspace";
import { useKeymap } from "../../keymap/keymap-provider";
import { useNotify } from "../../notifications/notifications";
import { activeWorktree, useActiveWorktree } from "../../open-with/active-worktree";
import { useHost, useRpcQuery } from "../../rpc/host";
import { CellDetail } from "../grid/cell-detail";
import type { GridCellMark, GridRect, ResultGridHandle } from "../grid/glide-grid";
import { ResultView } from "../grid/result-view";
import { useDbGridStrings } from "../grid/strings";
import { valueText } from "../grid/values";
import { useDbAccessPrompt } from "../safety/access-prompt";
import { ConnectionMark, TlsMark, connectionFrameStyle } from "../safety/connection-mark";
import {
  DB_SESSION_CHANGED_TOPIC,
  type DbActiveSession,
  useDbTabSession,
} from "../safety/db-status";
import { DbRefusalMessage, dbNoticeText, dbRefusal } from "../safety/messages";
import { useDbSafetyStrings } from "../safety/strings";
import { editableValue, sameColumnName, sameValue, valueFromInput } from "./changes";
import { type EditBuffer, isDefaultCell } from "./edit-buffer";
import { editFailureText } from "./failures";
import { type DbTableDataStrings, useDbTableDataStrings } from "./strings";
import { createTableDataStore, type TableDataSpec, type TableDataStore } from "./table-data-store";
import { foreignKeyWhere } from "./table-sql";
import { registerTableData, takeOpenedHere, tableDataWindow } from "./table-data-window";
import { SubmitPreviewDialog } from "./submit-preview";

/** 이 길이를 넘는 글은 상세 패널의 여러 줄 편집기로 연다. */
export const LONG_TEXT = 200;

/** 테이블 데이터 뿌리의 키맵 컨텍스트 키(그리드 안에서도 산다 — `ResultView` 의 `keymapContext`). */
export const TABLE_DATA_CONTEXT = "dbTableDataFocus";

const BUTTON =
  "flex h-[22px] items-center gap-1 rounded-sm border border-ui-border px-2 text-[12px] hover:bg-ui-hover disabled:opacity-50 disabled:hover:bg-transparent";
const INPUT =
  "h-[22px] min-w-0 rounded-sm border border-ui-border bg-ui-background px-1 font-mono text-[12px] disabled:opacity-50";

export interface TableDataDocumentProps {
  panelId: string;
  spec: TableDataSpec;
  onSpec(spec: TableDataSpec): void;
}

/** 열린 셀 편집기. */
interface Editing {
  row: number;
  column: number;
  text: string;
  /** 상세 패널의 여러 줄 편집기인가(아니면 제자리 칸). */
  long: boolean;
  /** 제자리 칸의 자리(문서 뿌리 기준). */
  rect?: GridRect;
}

/** 버퍼의 변경이 바뀔 때마다 다시 그린다. */
function usePending(buffer: EditBuffer | undefined): number {
  return useSyncExternalStore(
    useCallback((f) => buffer?.subscribe(f) ?? (() => {}), [buffer]),
    () => (buffer ? buffer.pendingVersion * 1_000_003 + buffer.version : -1),
  );
}

export function TableDataDocument(props: TableDataDocumentProps) {
  // 창의 테이블 데이터 기능(table-data-feature.tsx)이 프로젝트를 둔 뒤에 그린다.
  const projectId = useStore(tableDataWindow, (s) => s.projectId);
  if (projectId === undefined) return <div data-testid="db-table-data" className="h-full" />;
  return <TableDataBody {...props} projectId={projectId} />;
}

function TableDataBody({
  panelId,
  spec,
  onSpec,
  projectId,
}: TableDataDocumentProps & { projectId: string }) {
  const T = useDbTableDataStrings();
  const S = useDbSafetyStrings();
  const G = useDbGridStrings();
  const { client } = useHost();
  const worktree = useActiveWorktree();
  const workspace = useEditorWorkspace();
  const { registry } = useKeymap();
  const notify = useNotify();
  const access = useDbAccessPrompt();
  const root = useRef<HTMLDivElement>(null);
  const gridRef = useRef<ResultGridHandle | null>(null);
  const cursor = useRef<{ row: number; column: number }>(undefined);

  // 연결 목록(설정 읽기 — 연결을 열지 않는다): 표지 색 · 이름 · 드라이버.
  const list = useRpcQuery("db.connection.list", { projectId, worktree: worktree ?? null });
  const entry = list.data?.connections.find((c) => c.id === spec.connectionId);
  const color = entry?.config?.color ?? null;

  const latest = useRef({
    client,
    onSpec,
    driver: entry?.config?.driver,
    resolveAccess: (_error: unknown, _connectionId: string) => Promise.resolve(false),
  });
  latest.current = {
    ...latest.current,
    client,
    onSpec,
    driver: entry?.config?.driver,
    resolveAccess: (error, connectionId) =>
      client
        ? access.resolve(error, {
            calls: client,
            projectId,
            worktree: activeWorktree() ?? null,
            connectionId,
            connectionName: entry?.config?.name ?? connectionId,
          })
        : Promise.resolve(false),
  };

  const [store] = useState<TableDataStore>(() =>
    createTableDataStore(spec, {
      client: () => latest.current.client,
      projectId,
      worktree: activeWorktree,
      driver: () => latest.current.driver,
      onSpec: (next) => latest.current.onSpec(next),
      resolveAccess: (error, connectionId) => latest.current.resolveAccess(error, connectionId),
    }),
  );
  const state = useStore(store);
  const { opened, buffer, running, preview, submitting } = state;
  const pendingVersion = usePending(buffer);
  const pendingCount = buffer?.pendingCount ?? 0;
  const block = store.editBlock();
  const canEdit = block === undefined && !submitting;

  // 이 창에서 사람이 연 탭: 연결 목록을 받은 뒤 곧바로 연결한다(드라이버로 세션의 DB 를 정한다).
  const listed = list.data !== undefined;
  useEffect(() => {
    if (client && listed && takeOpenedHere(spec.tabId)) void store.connect();
  }, [client, listed, spec.tabId, store]);

  // 이 프로젝트의 세션 상태 · 알림.
  useEffect(() => {
    if (!client) return;
    const sub = client.subscribe({ projectId, topics: [DB_SESSION_CHANGED_TOPIC] }, (event) =>
      store.applySessionChanged(event.payload as unknown as DbSessionChanged),
    );
    sub.catch(() => {});
    return () => {
      sub.then((s) => s.unsubscribe()).catch(() => {});
    };
  }, [client, projectId, store]);

  // 상태바 `db` 항목(M3-19).
  const active = useMemo<DbActiveSession | undefined>(
    () =>
      opened && {
        session: opened.session,
        name: opened.name,
        production: opened.production,
        readOnly: opened.readOnly,
        color,
      },
    [opened, color],
  );
  useDbTabSession(panelId, active);

  // 닫기 전에 묻기(미제출 변경 — M3-15 의 `guardClose` 에 등록만 한다).
  useEffect(
    () =>
      workspace.guardClose({
        panelId,
        pending: () => store.needsCloseAnswer(),
        settle: async () => {
          const ask = tableDataWindow.getState().askDiscard;
          const count = store.getState().buffer?.pendingCount ?? 0;
          if (!ask) return false;
          const discard = await ask({ table: store.getState().spec.object.name, count });
          if (discard) store.revert();
          return discard;
        },
      }),
    [workspace, panelId, store],
  );

  // ── 셀 편집 ──
  const [editing, setEditing] = useState<Editing>();
  const editingRef = useRef(editing);
  editingRef.current = editing;

  const info = useCallback((title: string) => notify({ level: "info", title }), [notify]);

  const startEdit = useCallback(
    (cell: { row: number; column: number }) => {
      const b = store.getState().buffer;
      const blocked = store.editBlock();
      if (!b) return;
      if (blocked) {
        if (blocked !== "notConnected" && blocked !== "loading")
          info(T.block[blocked](errorMessage(store.getState().rowKey)));
        return;
      }
      if (b.isRemoved(cell.row)) return info(T.edit.deleted);
      if (!b.isAdded(cell.row) && store.keyColumns()?.includes(cell.column))
        return info(T.edit.key);
      const value = b.row(cell.row)?.[cell.column];
      if (!editableValue(value)) return info(T.edit.truncated);
      const text =
        value === undefined || value === "null" || isDefaultCell(value)
          ? ""
          : (valueText(value) ?? "");
      const json = value !== undefined && value !== "null" && value.json !== undefined;
      const long = json || /[\r\n]/.test(text) || text.length > LONG_TEXT;
      const bounds = gridRef.current?.bounds(cell);
      const base = root.current?.getBoundingClientRect();
      const rect =
        bounds && base
          ? {
              x: bounds.x - base.left,
              y: bounds.y - base.top,
              width: bounds.width,
              height: bounds.height,
            }
          : undefined;
      setEditing({ row: cell.row, column: cell.column, text, long, rect });
    },
    [store, info, T],
  );

  const commitEdit = useCallback(
    (value: DbValue | "input" | undefined) => {
      const now = editingRef.current;
      const b = store.getState().buffer;
      setEditing(undefined);
      editingRef.current = undefined;
      if (now && b && value !== undefined) {
        const original = b.original(now.row)?.[now.column];
        const next =
          value === "input" ? valueFromInput(now.text, b.columns[now.column], original) : value;
        b.setCell(now.row, now.column, next, sameValue);
      }
      gridRef.current?.focus();
    },
    [store],
  );

  // ── 행 추가 · 삭제 · 제출 · 되돌리기 ──
  const addRow = useCallback(() => {
    const b = store.getState().buffer;
    if (!b || store.editBlock()) return;
    commitEdit(editingRef.current ? "input" : undefined);
    b.addRow();
  }, [store, commitEdit]);

  const deleteRows = useCallback(() => {
    const b = store.getState().buffer;
    if (!b || store.editBlock()) return;
    const pick = gridRef.current?.pick();
    const rows = new Set<number>(pick?.rows ?? []);
    for (const r of pick?.rects ?? []) for (let y = r.y; y < r.y + r.height; y++) rows.add(y);
    if (rows.size === 0 && cursor.current) rows.add(cursor.current.row);
    if (rows.size > 0) b.toggleRemove([...rows]);
  }, [store]);

  const submit = useCallback(() => {
    if (editingRef.current) commitEdit("input");
    void store.submit();
  }, [store, commitEdit]);

  const revert = useCallback(() => {
    setEditing(undefined);
    store.revert();
  }, [store]);

  // 창의 액션이 부르는 자리.
  useEffect(
    () =>
      registerTableData({
        panelId,
        root: () => root.current,
        submit,
        revert,
        addRow,
        deleteRows,
        canEdit: () => store.editBlock() === undefined,
        hasPending: () => store.needsCloseAnswer(),
        dispose: () => void store.dispose(),
      }),
    [panelId, submit, revert, addRow, deleteRows, store],
  );

  // ── FK 따라가기 ──
  const followForeignKey = useCallback(
    async (cell: { row: number; column: number }) => {
      const { buffer: b, foreignKeys, spec: now } = store.getState();
      const dialect = store.getState().opened?.driver.dialect;
      const c = latest.current.client;
      if (!b || !dialect || !c) return;
      const name = b.columns[cell.column]?.name;
      const fk = name
        ? foreignKeys.find((k) => k.columns.some((col) => sameColumnName(dialect, col, name)))
        : undefined;
      if (!fk) return;
      const row = b.row(cell.row);
      if (!row) return;
      const pairs: { column: string; value: DbValue }[] = [];
      for (const [i, col] of fk.columns.entries()) {
        const at = b.columns.findIndex((m) => sameColumnName(dialect, m.name, col));
        const referenced = fk.referencedColumns[i];
        if (at < 0 || referenced === undefined) return;
        pairs.push({ column: referenced, value: row[at]! });
      }
      try {
        const where = await foreignKeyWhere(c, dialect, pairs);
        if (where === undefined) return info(T.edit.fkNull);
        void registry.run("db.tableData.open", {
          connectionId: now.connectionId,
          object: { ...fk.references, database: fk.references.database ?? now.object.database },
          where,
        });
      } catch (e) {
        notify({ level: "error", title: T.bar.error(e instanceof Error ? e.message : String(e)) });
      }
    },
    [store, registry, info, notify, T],
  );

  // ── 그리드 표지 ──
  const markLabels = T.mark;
  const cellMark = useMemo(() => {
    void pendingVersion;
    const b = buffer;
    if (!b) return undefined;
    return (row: number, column: number): GridCellMark | undefined => {
      const kind = b.mark(row, column);
      return kind ? { kind, label: markLabels[kind] } : undefined;
    };
  }, [buffer, pendingVersion, markLabels]);

  const dialect = opened?.driver.dialect ?? null;
  const name = opened?.name ?? entry?.config?.name ?? spec.connectionId;

  return (
    <div
      ref={root}
      tabIndex={-1}
      data-testid="db-table-data"
      data-keymap-context={TABLE_DATA_CONTEXT}
      data-session={opened?.session.sessionId}
      className="relative flex h-full min-h-0 flex-col bg-editor-background text-ui-foreground"
      style={opened ? connectionFrameStyle({ ...opened, color }) : undefined}
    >
      <TopBar
        store={store}
        T={T}
        canEdit={canEdit}
        pendingCount={pendingCount}
        onAddRow={addRow}
        onDeleteRows={deleteRows}
        onSubmit={submit}
        onRevert={revert}
        connectionName={name}
        color={color}
      />
      <Notices store={store} T={T} S={S} />
      <div className="flex min-h-0 flex-1">
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          {buffer ? (
            <ResultView
              buffer={buffer}
              notify={notify}
              localFilter={false}
              keymapContext={TABLE_DATA_CONTEXT}
              gridRef={gridRef}
              cellMark={cellMark}
              onActivate={startEdit}
              onCursor={(c) => {
                cursor.current = c;
              }}
              onCellClick={(c, m) => {
                if (m.mod) void followForeignKey(c);
              }}
              {...(client && dialect && { insert: { client, dialect, table: spec.object } })}
            />
          ) : (
            <p
              data-testid="db-table-data-empty"
              className="p-3 text-[12px] text-ui-muted-foreground"
            >
              {opened ? (running ? G.status.loading(0) : "") : T.bar.notConnected}
            </p>
          )}
        </div>
        {editing?.long && buffer && (
          <LongEditor
            key={`${editing.row}:${editing.column}`}
            T={T}
            G={G}
            column={buffer.columns[editing.column]?.name ?? ""}
            value={buffer.row(editing.row)?.[editing.column]}
            columnMeta={buffer.columns[editing.column]}
            text={editing.text}
            onText={(text) => setEditing({ ...editing, text })}
            onCommit={commitEdit}
          />
        )}
      </div>
      {editing && !editing.long && buffer && (
        <InlineEditor
          key={`${editing.row}:${editing.column}`}
          T={T}
          column={buffer.columns[editing.column]?.name ?? ""}
          rect={editing.rect}
          text={editing.text}
          onText={(text) => setEditing({ ...editing, text })}
          onCommit={commitEdit}
        />
      )}
      {preview && opened && (
        <SubmitPreviewDialog
          preview={preview.preview}
          connection={{
            name: opened.name,
            production: opened.production,
            readOnly: opened.readOnly,
            color,
          }}
          applying={submitting}
          failure={
            state.applyError === undefined
              ? undefined
              : editFailureText(state.applyError, T.fail, S.refusal)
          }
          onApply={() => void store.apply()}
          onCancel={() => store.closePreview()}
        />
      )}
      {access.element}
    </div>
  );
}

function errorMessage(rowKey: { failed: unknown } | { key: unknown } | undefined): string {
  if (!rowKey || !("failed" in rowKey)) return "";
  const e = rowKey.failed;
  return e instanceof Error ? e.message : String(e);
}

/** 위 막대. */
function TopBar({
  store,
  T,
  canEdit,
  pendingCount,
  onAddRow,
  onDeleteRows,
  onSubmit,
  onRevert,
  connectionName,
  color,
}: {
  store: TableDataStore;
  T: DbTableDataStrings;
  canEdit: boolean;
  pendingCount: number;
  onAddRow(): void;
  onDeleteRows(): void;
  onSubmit(): void;
  onRevert(): void;
  connectionName: string;
  color: string | null;
}) {
  const { opened, spec, busy, running, submitting } = useStore(store);
  const [where, setWhere] = useState(spec.where ?? "");
  const [orderBy, setOrderBy] = useState(spec.orderBy ?? "");
  // 헤더 클릭 정렬 · FK 로 조각이 바뀌면 칸도 따라간다.
  useEffect(() => setWhere(spec.where ?? ""), [spec.where]);
  useEffect(() => setOrderBy(spec.orderBy ?? ""), [spec.orderBy]);
  const disconnected = opened?.session.state.kind === "disconnected";
  const reading = !opened || disconnected || running || submitting || pendingCount > 0;
  const read = () => void store.query({ where, orderBy });
  const onKey = (e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter" && !e.nativeEvent.isComposing && !e.metaKey && !e.ctrlKey) {
      e.preventDefault();
      read();
    }
  };
  return (
    <div
      data-testid="db-table-data-bar"
      className="flex flex-wrap items-center gap-1 border-b border-ui-border bg-ui-panel px-2 py-1 text-[12px]"
    >
      <span className="flex items-center gap-1">
        <ConnectionMark
          name={connectionName}
          production={opened?.production ?? false}
          readOnly={opened?.readOnly ?? false}
          color={color}
        />
        {opened && <TlsMark tls={opened.server.tls} />}
      </span>
      {opened && !disconnected ? (
        <button
          type="button"
          className={BUTTON}
          data-testid="db-table-data-disconnect"
          disabled={busy || submitting}
          onClick={() => void store.disconnect()}
        >
          {T.bar.disconnect}
        </button>
      ) : (
        <button
          type="button"
          className={BUTTON}
          data-testid="db-table-data-connect"
          disabled={busy}
          onClick={() => {
            if (disconnected) void store.disconnect().then(() => store.connect());
            else void store.connect();
          }}
        >
          {T.bar.connect}
        </button>
      )}
      <span className="mx-1 h-4 border-l border-ui-border" aria-hidden />
      <label className="flex min-w-0 flex-1 items-center gap-1">
        <span className="font-mono text-ui-muted-foreground">{T.bar.where}</span>
        <input
          className={`${INPUT} flex-1`}
          data-testid="db-table-data-where"
          aria-label={T.bar.where}
          placeholder={T.bar.wherePlaceholder}
          spellCheck={false}
          value={where}
          onChange={(e) => setWhere(e.target.value)}
          onKeyDown={onKey}
        />
      </label>
      <label className="flex min-w-0 items-center gap-1">
        <span className="font-mono text-ui-muted-foreground">{T.bar.orderBy}</span>
        <input
          className={`${INPUT} w-48`}
          data-testid="db-table-data-order-by"
          aria-label={T.bar.orderBy}
          placeholder={T.bar.orderByPlaceholder}
          spellCheck={false}
          value={orderBy}
          onChange={(e) => setOrderBy(e.target.value)}
          onKeyDown={onKey}
        />
      </label>
      <button
        type="button"
        className={BUTTON}
        data-testid="db-table-data-read"
        title={pendingCount > 0 ? T.bar.pendingBlocksRead : T.bar.readHint}
        disabled={reading}
        onClick={read}
      >
        {T.bar.read}
      </button>
      <span className="mx-1 h-4 border-l border-ui-border" aria-hidden />
      <button
        type="button"
        className={BUTTON}
        data-testid="db-table-data-add-row"
        aria-label={T.bar.addRow}
        title={T.bar.addRow}
        disabled={!canEdit}
        onClick={onAddRow}
      >
        <Plus size={11} aria-hidden />
      </button>
      <button
        type="button"
        className={BUTTON}
        data-testid="db-table-data-delete-row"
        aria-label={T.bar.deleteRow}
        title={T.bar.deleteRow}
        disabled={!canEdit}
        onClick={onDeleteRows}
      >
        <Minus size={11} aria-hidden />
      </button>
      <button
        type="button"
        className={BUTTON}
        data-testid="db-table-data-submit"
        disabled={!canEdit || pendingCount === 0}
        onClick={onSubmit}
      >
        {T.bar.submit}
      </button>
      <button
        type="button"
        className={BUTTON}
        data-testid="db-table-data-revert"
        disabled={pendingCount === 0 || submitting}
        onClick={onRevert}
      >
        {T.bar.revert}
      </button>
      {pendingCount > 0 && (
        <span data-testid="db-table-data-pending" className="font-semibold">
          {T.bar.pending(pendingCount)}
        </span>
      )}
    </div>
  );
}

/** 위 막대 아래: 편집이 꺼진 까닭, 연결 상태, 거절 · 오류, 세션 알림, 적용 결과. */
function Notices({
  store,
  T,
  S,
}: {
  store: TableDataStore;
  T: DbTableDataStrings;
  S: ReturnType<typeof useDbSafetyStrings>;
}) {
  const state = useStore(store);
  const { opened, error, notice, applied, rowKey } = state;
  const block = store.editBlock();
  const lines: { key: string; text: string; warning?: boolean }[] = [];
  if (!opened) lines.push({ key: "idle", text: T.bar.notConnected });
  if (opened?.session.state.kind === "disconnected")
    lines.push({ key: "lost", text: T.bar.disconnected, warning: true });
  if (block && block !== "notConnected" && block !== "loading")
    lines.push({ key: "block", text: T.block[block](errorMessage(rowKey)) });
  if (notice) lines.push({ key: "notice", text: dbNoticeText(notice, S.notice), warning: true });
  if (applied)
    lines.push({ key: "applied", text: T.bar.applied(applied.statements, applied.rowsAffected) });
  const refused = error !== undefined && dbRefusal(error) !== undefined;
  if (error !== undefined && !refused)
    lines.push({
      key: "error",
      text: editFailureText(error, T.fail, S.refusal),
      warning: true,
    });
  if (lines.length === 0 && !refused) return null;
  return (
    <div
      data-testid="db-table-data-notices"
      className="flex flex-col gap-0.5 border-b border-ui-border bg-ui-panel px-2 py-1 text-[12px]"
    >
      {lines.map((line) => (
        <p
          key={line.key}
          data-notice={line.key}
          role={line.warning ? "alert" : undefined}
          className={line.warning ? "text-db-production" : "text-ui-muted-foreground"}
        >
          {line.text}
        </p>
      ))}
      {refused && <DbRefusalMessage error={error} />}
    </div>
  );
}

/** 제자리 편집 칸: Enter 적용 · Esc 취소 · 칸을 떠나면 적용. */
function InlineEditor({
  T,
  column,
  rect,
  text,
  onText,
  onCommit,
}: {
  T: DbTableDataStrings;
  column: string;
  rect: GridRect | undefined;
  text: string;
  onText(text: string): void;
  onCommit(value: DbValue | "input" | undefined): void;
}) {
  const input = useRef<HTMLInputElement>(null);
  const done = useRef(false);
  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, []);
  const finish = (value: DbValue | "input" | undefined) => {
    if (done.current) return;
    done.current = true;
    onCommit(value);
  };
  const style = rect
    ? {
        left: rect.x,
        top: rect.y,
        width: Math.max(rect.width + 48, 160),
        height: Math.max(rect.height, 22),
      }
    : { left: 8, top: 40, width: 320, height: 24 };
  return (
    <div
      data-testid="db-table-data-inline-editor"
      className="absolute z-10 flex items-stretch border border-ui-accent bg-ui-background shadow"
      style={style}
    >
      <input
        ref={input}
        aria-label={T.edit.label(column)}
        spellCheck={false}
        className="min-w-0 flex-1 bg-transparent px-1 font-mono text-[12px] outline-none"
        value={text}
        onChange={(e) => onText(e.target.value)}
        onBlur={(e) => {
          // [NULL] 을 누르러 가는 포커스 이동은 적용이 아니다.
          if (e.currentTarget.parentElement?.contains(e.relatedTarget as Node | null)) return;
          finish("input");
        }}
        onKeyDown={(e) => {
          if (e.nativeEvent.isComposing) return;
          if (e.key === "Enter" && !e.metaKey && !e.ctrlKey) {
            e.preventDefault();
            finish("input");
          } else if (e.key === "Escape") {
            e.preventDefault();
            e.stopPropagation();
            finish(undefined);
          }
        }}
      />
      <button
        type="button"
        data-testid="db-table-data-edit-null"
        className="border-l border-ui-border px-1 text-[11px] text-ui-muted-foreground hover:bg-ui-hover"
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => finish("null")}
      >
        {T.edit.null}
      </button>
    </div>
  );
}

/** 상세 패널의 여러 줄 편집기(JSON · 여러 줄 · 긴 글). */
function LongEditor({
  T,
  G,
  column,
  value,
  columnMeta,
  text,
  onText,
  onCommit,
}: {
  T: DbTableDataStrings;
  G: ReturnType<typeof useDbGridStrings>;
  column: string;
  value: DbValue | undefined;
  columnMeta: Parameters<typeof CellDetail>[0]["column"];
  text: string;
  onText(text: string): void;
  onCommit(value: DbValue | "input" | undefined): void;
}) {
  const area = useRef<HTMLTextAreaElement>(null);
  useEffect(() => area.current?.focus(), []);
  return (
    <aside
      data-testid="db-table-data-long-editor"
      aria-label={T.edit.label(column)}
      className="flex w-96 min-w-0 shrink-0 flex-col gap-1 border-l border-ui-border p-2"
    >
      <div className="text-[12px] font-semibold">{T.edit.label(column)}</div>
      <textarea
        ref={area}
        aria-label={T.edit.label(column)}
        spellCheck={false}
        className="min-h-0 flex-1 resize-none rounded-sm border border-ui-border bg-ui-background p-1 font-mono text-[12px]"
        value={text}
        onChange={(e) => onText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape" && !e.nativeEvent.isComposing) {
            e.preventDefault();
            e.stopPropagation();
            onCommit(undefined);
          }
        }}
      />
      <div className="flex justify-end gap-1">
        <button
          type="button"
          className={BUTTON}
          data-testid="db-table-data-edit-null"
          onClick={() => onCommit("null")}
        >
          {T.edit.null}
        </button>
        <button type="button" className={BUTTON} onClick={() => onCommit(undefined)}>
          {T.edit.cancel}
        </button>
        <button
          type="button"
          className={BUTTON}
          data-testid="db-table-data-edit-apply"
          onClick={() => onCommit("input")}
        >
          {T.edit.apply}
        </button>
      </div>
      <details className="min-h-0 text-[12px]">
        <summary className="cursor-default text-ui-muted-foreground">{G.detail.title}</summary>
        <div className="max-h-48 overflow-auto">
          <CellDetail value={value} column={columnMeta} T={G} />
        </div>
      </details>
    </aside>
  );
}
