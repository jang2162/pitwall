// 「에이전트 활동」 패널의 호출 로그 모델(16-m5-plan M5-13). React 없이 `mcp.audit.list` 의 쪽들을 쥐고, 새 행 · 바뀐 행을 다시 읽는다.
//
// - 행은 최신부터(id 내림차순)다. 첫 쪽을 읽고, 화면이 끝에 닿으면 다음 쪽(`nextCursor`)을 읽는다(`loadMore`).
// - 새로 읽기(`refresh(from)`)는 맨 위부터 쪽을 읽어 id 가 `from` 이하인 행까지 덮는다. 읽은 쪽들은 그 id 구간의 정답이라 구간 안의
//   옛 행을 갈아 끼운다 — 새 행이 맨 위에 붙고, 선기록된 행(`started`)이 끝나 바뀐 것도 그 자리에서 바뀌고, 결과 거르기에 더는 맞지
//   않는 행은 빠진다. 받아 둔 행보다 아래로는 내려가지 않는다(아직 읽지 않은 행은 스크롤로 읽는다).
// - `mcp.activity` 는 행 id 를 싣는다(새 행, 그리고 선기록된 행이 끝날 때 같은 id 로 한 번 더 — crates/pw-mcp/src/server.rs
//   `audit_writer`). 이벤트가 몰려도 새로 읽기는 하나씩만 돈다 — 도는 중에 온 것은 가장 작은 id 하나로 모아 끝난 뒤 한 번 더 읽는다.
// - 다른 프로세스(헤드리스 `pitwall mcp` · `pitwall tool`)가 쓴 행은 이벤트가 없다 — 창이 앞으로 올 때 `refreshAll` 이 맨 위부터
//   받아 둔 가장 최신 행까지, 그리고 받아 둔 행 중 아직 `started` 인 가장 오래된 행까지 다시 읽는다(carry-notes R8).
// - 거르기(결과 · 툴)는 Host 가 거른다(`mcp.audit.list` 의 `outcome` · `tool`). 바꾸면 처음부터 다시 읽는다. 클라이언트 거르기는
//   RPC 에 칸이 없어 화면이 받은 행에서 거른다(agent-activity.tsx).
// - 처음부터 다시 읽을 때(`reset` — 거르기를 바꿈 · 기록 지우기 뒤)는 세대를 올린다. 옛 세대의 응답(지우기 전에 떠난 다음 쪽 등)은
//   버린다 — 지운 뒤 커서가 엉뚱한 행을 붙이지 않는다. 행 id 는 다시 쓰이지 않아(M5-11) 커서가 남아도 새 행을 가리키지는 않지만,
//   지운 행을 다시 그리지 않게 응답째 버린다.

import type { AuditOutcome, AuditRow, McpAuditListParams, McpAuditPage } from "@pitwall/rpc-client";
import { createStore, type StoreApi } from "zustand/vanilla";

/** 한 쪽의 행 수(`mcp.audit.list` 의 `limit` 상한은 500). */
export const AUDIT_PAGE = 200;

export interface AuditFilter {
  readonly outcome: AuditOutcome | null;
  readonly tool: string | null;
}

export interface AuditLogState {
  /** 최신부터(id 내림차순). */
  readonly rows: readonly AuditRow[];
  /** 가장 오래된 쪽을 읽은 뒤 더 읽을 것이 없다. */
  readonly end: boolean;
  /** 첫 쪽을 아직 받지 못했다. */
  readonly loading: boolean;
  /** 마지막 읽기의 실패(다음 읽기가 성공하면 지운다). */
  readonly error: string | undefined;
  readonly filter: AuditFilter;
}

export type AuditFetch = (params: McpAuditListParams) => Promise<McpAuditPage>;

export interface AuditLog {
  readonly store: StoreApi<AuditLogState>;
  /** 처음부터 다시 읽는다. 떠 있는 읽기의 응답은 버린다. */
  reset(): void;
  /** 거르기를 바꾸고 처음부터 다시 읽는다. */
  setFilter(filter: AuditFilter): void;
  /** 다음(더 오래된) 쪽. 읽는 중이거나 끝이면 아무것도 하지 않는다. */
  loadMore(): void;
  /** `mcp.activity` 로 받은 행 id 들 — 그 행까지 맨 위부터 다시 읽는다. */
  activity(ids: readonly number[]): void;
  /** 창이 앞으로 왔다 — 다른 프로세스가 쓰거나 끝낸 행을 다시 읽는다. */
  refreshAll(): void;
  /** 떠 있는 읽기를 기다린다(시험 · 지우기 뒤). */
  idle(): Promise<void>;
  dispose(): void;
}

const NONE: AuditFilter = { outcome: null, tool: null };

function message(e: unknown) {
  return e instanceof Error ? e.message : String(e);
}

/**
 * 읽은 쪽들(`fetched`, id 내림차순)이 정답인 구간 `[low, high)` 의 행을 갈아 끼운다. 구간 밖의 옛 행은 그대로 둔다.
 * `high` 가 `Infinity` 면 맨 위부터, `low` 가 `-Infinity` 면 맨 끝까지다.
 */
export function mergeRows(
  rows: readonly AuditRow[],
  fetched: readonly AuditRow[],
  low: number,
  high: number,
): AuditRow[] {
  const kept = rows.filter((r) => r.id < low || r.id >= high);
  const seen = new Set(fetched.map((r) => r.id));
  return [...fetched, ...kept.filter((r) => !seen.has(r.id))].sort((a, b) => b.id - a.id);
}

export function createAuditLog(options: {
  fetch: AuditFetch;
  projectId: string;
  page?: number;
}): AuditLog {
  const page = options.page ?? AUDIT_PAGE;
  const store = createStore<AuditLogState>()(() => ({
    rows: [],
    end: false,
    loading: true,
    error: undefined,
    filter: NONE,
  }));
  /** 처음부터 다시 읽을 때마다 오른다. 응답은 떠날 때의 세대와 같을 때만 반영한다. */
  let generation = 0;
  /** 다음 쪽의 커서와 그 쪽이 시작하는 자리(앞 쪽의 마지막 행 id — 그보다 작은 id 가 다음 쪽이다). */
  let cursor: { value: string; below: number } | undefined;
  let moreInFlight = false;
  let refreshing = false;
  /** 새로 읽는 중에 온 요청 — 가장 작은 id 하나로 모은다. `undefined` 면 없음. */
  let pending: number | undefined;
  const inflight = new Set<Promise<unknown>>();
  let disposed = false;

  const track = <T>(p: Promise<T>) => {
    inflight.add(p);
    void p.finally(() => inflight.delete(p)).catch(() => {});
    return p;
  };

  const params = (next: string | null): McpAuditListParams => {
    const { filter } = store.getState();
    return {
      projectId: options.projectId,
      cursor: next,
      limit: page,
      outcome: filter.outcome,
      tool: filter.tool,
    };
  };

  const fail = (e: unknown) => store.setState({ error: message(e), loading: false });

  const reset = () => {
    generation += 1;
    const mine = generation;
    cursor = undefined;
    moreInFlight = false;
    refreshing = false;
    pending = undefined;
    store.setState({ rows: [], end: false, loading: true, error: undefined });
    void track(
      options.fetch(params(null)).then(
        (got) => {
          if (disposed || mine !== generation) return;
          setCursor(got);
          store.setState({
            rows: got.entries,
            end: got.nextCursor === null,
            loading: false,
            error: undefined,
          });
          // 첫 쪽이 떠 있는 동안 온 이벤트.
          drain();
        },
        (e: unknown) => {
          if (disposed || mine !== generation) return;
          fail(e);
        },
      ),
    );
  };

  const setCursor = (got: McpAuditPage) => {
    const last = got.entries.at(-1);
    cursor =
      got.nextCursor !== null && last ? { value: got.nextCursor, below: last.id } : undefined;
  };

  const loadMore = () => {
    const state = store.getState();
    if (disposed || moreInFlight || state.loading || state.end || !cursor) return;
    const mine = generation;
    const from = cursor;
    moreInFlight = true;
    void track(
      options.fetch(params(from.value)).then(
        (got) => {
          if (disposed || mine !== generation) return;
          moreInFlight = false;
          // 새로 읽기가 그새 커서를 지나 내려갔으면 이 쪽은 낡았다 — 버리고 지금 커서로 다시 읽게 둔다.
          if (cursor !== from) return;
          const last = got.entries.at(-1);
          const low = got.nextCursor === null || !last ? -Infinity : last.id;
          setCursor(got);
          store.setState((s) => ({
            rows: mergeRows(s.rows, got.entries, low, from.below),
            end: got.nextCursor === null,
            error: undefined,
          }));
        },
        (e: unknown) => {
          if (disposed || mine !== generation) return;
          moreInFlight = false;
          fail(e);
        },
      ),
    );
  };

  /** 맨 위부터 id 가 `from` 이하인 행이 나올 때까지(받아 둔 행 아래로는 내려가지 않는다). */
  const refresh = (from: number) => {
    if (disposed) return;
    if (refreshing || store.getState().loading) {
      pending = pending === undefined ? from : Math.min(pending, from);
      return;
    }
    refreshing = true;
    const mine = generation;
    const run = async () => {
      const { rows, end } = store.getState();
      const oldest = rows.at(-1)?.id;
      // 받아 둔 행이 없으면 첫 쪽 하나만. 받아 둔 행보다 아래는 보지 않는다(끝까지 받았으면 거기까지가 전부다).
      const stop = oldest === undefined ? Infinity : end ? from : Math.max(from, oldest);
      const fetched: AuditRow[] = [];
      let next: string | null = null;
      let got: McpAuditPage;
      do {
        got = await options.fetch(params(next));
        if (disposed || mine !== generation) return;
        fetched.push(...got.entries);
        next = got.nextCursor;
      } while (next !== null && (fetched.at(-1)?.id ?? -Infinity) > stop);
      const last = fetched.at(-1);
      const reachedEnd = got.nextCursor === null;
      const low = reachedEnd || !last ? -Infinity : last.id;
      // 받아 둔 끝(커서)보다 아래까지 읽었으면 커서를 이 읽기의 것으로 옮긴다.
      if (reachedEnd) cursor = undefined;
      else if (last && (cursor === undefined || last.id < cursor.below) && !end)
        cursor = { value: got.nextCursor!, below: last.id };
      store.setState((s) => ({
        rows: mergeRows(s.rows, fetched, low, Infinity),
        end: reachedEnd ? true : s.end,
        error: undefined,
      }));
    };
    void track(
      run().then(
        () => {
          if (mine !== generation) return;
          refreshing = false;
          drain();
        },
        (e: unknown) => {
          if (disposed || mine !== generation) return;
          refreshing = false;
          fail(e);
          drain();
        },
      ),
    );
  };

  const drain = () => {
    if (pending === undefined) return;
    const from = pending;
    pending = undefined;
    refresh(from);
  };

  const refreshAll = () => {
    const { rows } = store.getState();
    const newest = rows[0]?.id;
    if (newest === undefined) {
      // 아무것도 받지 못했으면(빈 기록 · 첫 읽기 실패) 첫 쪽부터.
      if (!store.getState().loading) reset();
      return;
    }
    let from = newest;
    for (const r of rows) if (r.outcome === "started" && r.id < from) from = r.id;
    refresh(from);
  };

  reset();

  return {
    store,
    reset,
    setFilter(filter) {
      store.setState({ filter });
      reset();
    },
    loadMore,
    activity(ids) {
      if (ids.length === 0) return;
      refresh(Math.min(...ids));
    },
    refreshAll,
    async idle() {
      while (inflight.size > 0) await Promise.allSettled(inflight);
    },
    dispose() {
      disposed = true;
    },
  };
}
