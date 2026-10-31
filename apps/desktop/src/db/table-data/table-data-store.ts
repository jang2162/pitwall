// 테이블 데이터 탭 하나의 세션 · 읽기 · 편집(14-m3-plan.md M3-17). 화면(table-data-document.tsx)과 떨어진 상태 기계라 창 시험이
// RPC 순서를 본다.
//
// - **연결은 사람이 누를 때만**(결정 M3-D13): 되살린 탭은 아무 RPC 도 부르지 않는다 — [연결] 이 `db.session.open` 을 부른다. 이
//   창에서 사람이 연 탭(탐색기의 더블클릭 · FK ⌘+클릭)은 연 동작이 곧 누름이라 곧바로 연결한다(화면이 `connect()` 를 부른다).
//   세션은 자동 커밋이고 모드를 바꾸지 않는다(위 막대에 TX 가 없다 — 10-ui.md 「에디터 영역 문서 타입」). `3101` · `3102` 면 DB 접근
//   묻기(M3-13)로 넣고 다시 연다. PG 는 테이블의 DB 로 세션을 연다(편집의 세 부분 이름은 세션의 DB 여야 한다 — M3-12 ⓑ).
// - **읽기**: `SELECT * FROM <테이블> WHERE … ORDER BY …`(table-sql.ts)를 `db.query.execute { queryOnly: true }` 로 — 조회 문장
//   하나가 아니면 Host 가 거절하고(`notAQuery`) 그 거절이 위 막대 아래에 보인다. 화면은 조각을 검사하지 않는다. 헤더 클릭 정렬은
//   ORDER BY 칸을 그 열로 바꿔 다시 읽는다. 제출하지 않은 변경이 있으면 다시 읽지 않는다(읽으면 변경이 가리키는 행이 사라진다).
// - **편집**(키 있는 테이블만, 결정 M3-D9): 연결하면 `db.edit.rowKey` 로 키를 받는다 — `null` 이면 편집을 끄고 까닭을 보인다.
//   read-only 연결 · 뷰도 끈다(Host 가 거절하는 것을 미리 끈다 — 거절의 정본은 Host). [제출] 은 `db.edit.preview` 를 불러 미리보기
//   (Host 가 만든 문장 글 · 등급 · `previewHash`)를 띄우고, 미리보기의 [적용] 이 같은 `changes` · 같은 `previewHash` 로
//   `db.edit.apply` 를 부른다 — 그것이 결정 M3-D20 의 확인이다(M3-12 ⓐ). 적용되면 변경을 비우고 다시 읽는다. 실패하면 변경은 그대로
//   두고 까닭을 보인다(영향 행 수 · DB 오류 · 미리보기 뒤 바뀜).
// - **FK**(⌘+클릭): 연결하면 `db.schema.foreignKeys` 로 FK 를 받아 둔다(메타데이터용 연결 — 세션과 따로).
// - **닫기 전에 묻기**: 제출하지 않은 변경이 있으면 `needsCloseAnswer()` 가 참 — 묻는 것은 창의 몫(table-data-feature.tsx).

import type {
  DbEditApplied,
  DbEditChanges,
  DbEditPreview,
  DbRowKey,
  DbSessionChanged,
  DbSessionNotice,
  DbSessionOpened,
  DriverKind,
  ForeignKeyInfo,
  ObjectRef,
  RpcClient,
} from "@pitwall/rpc-client";
import { createStore, type StoreApi } from "zustand/vanilla";

import { QueryRun } from "../grid/query-stream";
import type { SortSpec } from "../grid/result-buffer";
import { buildChanges, keyColumnsOf } from "./changes";
import { EditBuffer } from "./edit-buffer";
import { orderByColumn, tableSelect } from "./table-sql";

/** 테이블 데이터 탭의 저장되는 것(탭을 복원해도 같다). */
export interface TableDataSpec {
  tabId: string;
  connectionId: string;
  object: ObjectRef;
  /** WHERE 조각(없으면 빈 글). */
  where?: string;
  /** ORDER BY 조각(없으면 빈 글). */
  orderBy?: string;
}

export interface TableDataDeps {
  client(): RpcClient | undefined;
  projectId: string;
  /** 활성 워크트리의 꼭대기. */
  worktree(): string | undefined;
  /** 연결 설정의 드라이버(연결 목록 — 연결을 열지 않는다). 모르면 `undefined`. */
  driver(): DriverKind | undefined;
  /** 저장되는 것이 바뀌었다(WHERE · ORDER BY). */
  onSpec(spec: TableDataSpec): void;
  /** `3101` · `3102` 를 묻고 넣었으면 `true`(M3-13 `useDbAccessPrompt`). */
  resolveAccess?(error: unknown, connectionId: string): Promise<boolean>;
}

/** 편집이 꺼진 까닭(켜졌으면 없다). */
export type EditBlock =
  | "notConnected"
  | "loading"
  | "readOnly"
  | "noKey"
  | "keyNotInResult"
  | "failed";

export interface TableDataState {
  spec: TableDataSpec;
  opened?: DbSessionOpened;
  /** 여는 · 닫는 중. */
  busy: boolean;
  /** 마지막 오류(거절 · DB 오류 · 연결 실패) — 다음 동작이 지운다. */
  error?: unknown;
  notice?: DbSessionNotice;
  /** 지금 결과(읽을 때마다 새것). */
  buffer?: EditBuffer;
  running: boolean;
  /** `db.edit.rowKey` 의 답. 받기 전이면 `undefined`. */
  rowKey?: { key: DbRowKey | null } | { failed: unknown };
  foreignKeys: ForeignKeyInfo[];
  /** 떠 있는 미리보기(그 `changes` 로 적용한다). */
  preview?: { preview: DbEditPreview; changes: DbEditChanges };
  /** 미리보기를 받는 · 적용하는 중. */
  submitting: boolean;
  /** 마지막 적용 실패(미리보기 다이얼로그 · 위 막대에 보인다). */
  applyError?: unknown;
  /** 마지막 적용 결과. */
  applied?: DbEditApplied;
}

export type TableDataStore = StoreApi<TableDataState> & ReturnType<typeof tableDataActions>;

const ACCESS_RETRIES = 4;

export function createTableDataStore(spec: TableDataSpec, deps: TableDataDeps): TableDataStore {
  const store = createStore<TableDataState>()(() => ({
    spec,
    busy: false,
    running: false,
    foreignKeys: [],
    submitting: false,
  }));
  return Object.assign(store, tableDataActions(store, deps));
}

/** 정렬 상태(헤더 화살표) — ORDER BY 칸이 열 하나의 `<열> ASC|DESC` 일 때만. */
function sortOf(buffer: EditBuffer, orderBy: string): SortSpec | undefined {
  const m =
    /^\s*(?:"((?:[^"]|"")+)"|`((?:[^`]|``)+)`|([A-Za-z_][A-Za-z0-9_$]*))\s+(ASC|DESC)\s*$/i.exec(
      orderBy,
    );
  if (!m) return undefined;
  const name = m[1]?.replaceAll('""', '"') ?? m[2]?.replaceAll("``", "`") ?? m[3]!;
  const column = buffer.columns.findIndex((c) => c.name === name);
  if (column < 0) return undefined;
  return { column, direction: m[4]!.toLowerCase() === "asc" ? "asc" : "desc" };
}

function tableDataActions(store: StoreApi<TableDataState>, deps: TableDataDeps) {
  const get = store.getState;
  const set = store.setState;
  const client = (): RpcClient => {
    const c = deps.client();
    if (!c) throw new Error("the window is not connected to its Host");
    return c;
  };
  let abort: AbortController | undefined;
  let disposed = false;
  /** 읽기마다 오른다 — 늦게 끝난 앞 읽기가 뒤 읽기의 상태를 덮지 않게. */
  let readToken = 0;

  const updateSpec = (change: Partial<TableDataSpec>) => {
    const next = { ...get().spec, ...change };
    set({ spec: next });
    deps.onSpec(next);
  };

  const dialect = () => get().opened?.driver.dialect ?? undefined;

  const actions = {
    /** [연결]: 세션을 열고 키 · FK 를 받고 읽는다. */
    async connect() {
      const { spec, opened, busy } = get();
      if (opened || busy) return;
      set({ busy: true, error: undefined, notice: undefined });
      // 다른 DB 의 테이블이면 그 DB 로 연다(PG — 세 부분 이름은 세션의 DB 여야 한다). SQLite 는 DB 를 고르지 않는다.
      const driver = deps.driver();
      const database = driver === "sqlite" ? null : (spec.object.database ?? null);
      const params = {
        projectId: deps.projectId,
        worktree: deps.worktree() ?? null,
        connectionId: spec.connectionId,
        database,
      };
      try {
        for (let attempt = 0; ; attempt++) {
          try {
            const next = await client().call("db.session.open", params);
            if (disposed) {
              await client()
                .call("db.session.close", { sessionId: next.session.sessionId })
                .catch(() => undefined);
              return;
            }
            set({ opened: next, rowKey: undefined, foreignKeys: [] });
            break;
          } catch (e) {
            if (
              disposed ||
              attempt >= ACCESS_RETRIES ||
              !(await deps.resolveAccess?.(e, params.connectionId))
            )
              throw e;
          }
        }
      } catch (e) {
        set({ error: e });
        return;
      } finally {
        set({ busy: false });
      }
      void actions.loadKey();
      void actions.loadForeignKeys();
      await actions.read();
    },

    /** 키(편집할 수 있는 테이블인가). */
    async loadKey() {
      const opened = get().opened;
      if (!opened) return;
      try {
        const got = await client().call("db.edit.rowKey", {
          sessionId: opened.session.sessionId,
          table: get().spec.object,
        });
        if (get().opened === opened) set({ rowKey: { key: got.key } });
      } catch (e) {
        if (get().opened === opened) set({ rowKey: { failed: e } });
      }
    },

    /** FK(⌘+클릭으로 따라간다). 실패해도 편집 · 읽기는 그대로다. */
    async loadForeignKeys() {
      const opened = get().opened;
      if (!opened) return;
      try {
        const got = await client().call("db.schema.foreignKeys", {
          projectId: deps.projectId,
          worktree: deps.worktree() ?? null,
          connectionId: get().spec.connectionId,
          object: get().spec.object,
        });
        if (get().opened === opened) set({ foreignKeys: got.items });
      } catch {
        // FK 를 못 읽으면 ⌘+클릭이 아무것도 하지 않을 뿐이다.
      }
    },

    /** 세션을 닫는다. */
    async disconnect() {
      const opened = get().opened;
      if (!opened) return;
      abort?.abort();
      set({ opened: undefined, notice: undefined, running: false, preview: undefined });
      try {
        await client().call("db.session.close", { sessionId: opened.session.sessionId });
      } catch (e) {
        set({ error: e });
      }
    },

    /** 지금 조각으로 다시 읽는다(제출하지 않은 변경은 부르는 쪽이 본다 — `query` · 적용 뒤의 `revert`). */
    async read(): Promise<boolean> {
      const { opened, spec } = get();
      const d = dialect();
      if (!opened || !d || opened.session.state.kind === "disconnected") return false;
      abort?.abort();
      const controller = new AbortController();
      abort = controller;
      const token = ++readToken;
      const sql = tableSelect(d, spec.object, {
        where: spec.where ?? "",
        orderBy: spec.orderBy ?? "",
      });
      const next = new EditBuffer();
      next.onSort = (sort) => void actions.sortBy(sort);
      const query = new QueryRun(() => next);
      get().buffer?.dispose();
      set({ buffer: next, running: true, error: undefined, applied: undefined });
      try {
        await query.run(
          client(),
          { sessionId: opened.session.sessionId, sql, queryOnly: true },
          controller.signal,
        );
        if (token === readToken) next.showSort(sortOf(next, spec.orderBy ?? ""));
        return true;
      } catch (e) {
        if (token === readToken && !controller.signal.aborted) set({ error: e });
        return false;
      } finally {
        if (abort === controller) abort = undefined;
        if (token === readToken) set({ running: false });
      }
    },

    /** WHERE · ORDER BY 를 바꾸고 다시 읽는다. 제출하지 않은 변경이 있으면 바꾸지 않고 `false`. */
    async query(change: { where?: string; orderBy?: string }): Promise<boolean> {
      if ((get().buffer?.pendingCount ?? 0) > 0) return false;
      updateSpec(change);
      return actions.read();
    },

    /** 헤더 클릭 정렬: ORDER BY 를 그 열로(풀면 비운다). */
    async sortBy(sort: SortSpec | undefined) {
      const d = dialect();
      const buffer = get().buffer;
      if (!d || !buffer) return;
      const column = sort && buffer.columns[sort.column];
      await actions.query({ orderBy: column ? orderByColumn(d, column.name, sort.direction) : "" });
    },

    /** 편집이 꺼진 까닭(켜졌으면 `undefined`). */
    editBlock(): EditBlock | undefined {
      const { opened, rowKey, buffer, running } = get();
      if (!opened) return "notConnected";
      if (opened.readOnly) return "readOnly";
      if (!rowKey || running || !buffer) return "loading";
      if ("failed" in rowKey) return "failed";
      if (!rowKey.key) return "noKey";
      if (!actions.keyColumns()) return "keyNotInResult";
      return undefined;
    },

    /** 결과에서 키 열의 번호(편집이 켜졌을 때). */
    keyColumns(): number[] | undefined {
      const { rowKey, buffer } = get();
      const d = dialect();
      if (!rowKey || "failed" in rowKey || !rowKey.key || !buffer || !d) return undefined;
      return keyColumnsOf(d, buffer.columns, rowKey.key.columns);
    },

    /** [제출]: 미리보기를 받는다. */
    async submit() {
      const { opened, buffer, submitting } = get();
      const keys = actions.keyColumns();
      if (!opened || !buffer || !keys || submitting || actions.editBlock()) return;
      if (buffer.pendingCount === 0) return;
      const changes = buildChanges(buffer, keys);
      set({ submitting: true, applyError: undefined, error: undefined });
      try {
        const preview = await client().call("db.edit.preview", {
          sessionId: opened.session.sessionId,
          table: get().spec.object,
          changes,
        });
        set({ preview: { preview, changes } });
      } catch (e) {
        set({ error: e });
      } finally {
        set({ submitting: false });
      }
    },

    /** 미리보기의 [적용]: 같은 변경 · 같은 `previewHash` 로 적용한다. 되면 변경을 비우고 다시 읽는다. */
    async apply(): Promise<boolean> {
      const { opened, preview, buffer, submitting } = get();
      if (!opened || !preview || !buffer || submitting) return false;
      set({ submitting: true, applyError: undefined });
      try {
        const applied = await client().call("db.edit.apply", {
          sessionId: opened.session.sessionId,
          table: get().spec.object,
          changes: preview.changes,
          previewHash: preview.preview.previewHash,
        });
        set({ preview: undefined, applied });
        buffer.revert();
        await actions.read();
        set({ applied });
        return true;
      } catch (e) {
        set({ applyError: e });
        return false;
      } finally {
        set({ submitting: false });
      }
    },

    /** 미리보기를 닫는다(적용하지 않는다). */
    closePreview() {
      set({ preview: undefined, applyError: undefined });
    },

    /** [되돌리기]: 제출하지 않은 변경을 모두 버린다. */
    revert() {
      get().buffer?.revert();
      set({ applyError: undefined });
    },

    /** `db.sessionChanged`. 이 탭의 세션 것만. */
    applySessionChanged(event: DbSessionChanged) {
      const opened = get().opened;
      if (!opened || opened.session.sessionId !== event.session.sessionId) return;
      if (event.notice?.kind === "unattendedRolledBack") {
        set({ opened: undefined, notice: event.notice });
        return;
      }
      set({
        opened: { ...opened, session: event.session },
        ...(event.notice && { notice: event.notice }),
      });
    },

    /** 닫기 전에 물을 것(제출하지 않은 변경)이 있다. */
    needsCloseAnswer(): boolean {
      return (get().buffer?.pendingCount ?? 0) > 0;
    },

    /** 탭이 닫혔다: 읽기를 멈추고 세션을 닫는다. */
    async dispose() {
      disposed = true;
      abort?.abort();
      get().buffer?.dispose();
      await actions.disconnect();
    },
  };
  return actions;
}
