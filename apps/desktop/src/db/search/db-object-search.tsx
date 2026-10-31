// Search Everywhere 의 「DB 객체」 탭(⇧⇧, 10-ui §4.2 의 SE 행, 14-m3-plan.md M3-20).
//
// - 원천은 Host 의 스키마 캐시에 이미 든 테이블 · 뷰 이름이다(`db.schema.cached` — M3-10 의 캐시). Host 는 연결을 열지 않고 드라이버를
//   부르지 않는다(결정 M3-D13: 연결은 사람이 누를 때만). 팝업을 열 때마다 한 번 읽고, 글자마다 묻지 않고 창에서 거른다(액션 탭과
//   같은 자모 단위 비교 — 이름, 그다음 스키마 · DB · 연결 이름). 미신뢰 프로젝트에서는 비운다(Host 가 `untrusted` 로 거절한다).
// - 고르면(Enter · 클릭) DB 탐색기 툴 윈도우를 띄우고 그 객체를 트리에서 고른다(`db.explorer.reveal` — 탐색기가 처음 그려지는 중이면
//   등록될 때까지 잠시 기다린다). 그 연결이 이 창에서 연결돼 있지 않으면 연결 행만 고른다(탐색기의 판정).
// - `⌘↩`(Win/Linux `Ctrl+Enter`)는 테이블 데이터(`db.tableData.open { connectionId, object }` — M3-17)를 연다. 그 액션이 등록되지
//   않았으면 `⌘↩` 는 아무것도 하지 않는다(Enter 로 새지 않는다). 팝업 안에서는 키맵이 꺼져 있으므로 키를 여기서 본다.
// SE 팝업은 KeymapProvider 아래(이 창의 컴포넌트 밖)에 그려지므로 결과 컴포넌트는 클로저로 이 창의 것을 받는다.

import type { DbCachedObject, DbSchemaCached } from "@pitwall/rpc-client";
import { ERROR_CODE, RpcCallError } from "@pitwall/rpc-client";
import type { ActionRegistry } from "@pitwall/keymap";
import { Command, useCommandState } from "cmdk";
import type { DockviewApi } from "dockview-react";
import {
  type ReactNode,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";

import { useAppStrings } from "../../i18n/app-strings";
import { useKeymap, useSearchSource } from "../../keymap/keymap-provider";
import { showPanel } from "../../layout/project-layout";
import { useActiveWorktree } from "../../open-with/active-worktree";
import { useHost } from "../../rpc/host";
import {
  GROUP_HEADING_CLASS,
  MoreItem,
  type SearchResultsProps,
  type SearchSource,
} from "../../search-everywhere/search-everywhere";
import { SEARCH_ITEM_CLASS, searchFilter } from "../../ui/search-popup";
import { EXPLORER_ACTIONS, EXTERNAL_ACTIONS } from "../explorer/explorer-actions";

/** DB 객체 탭 하나가 보이는 수. */
export const DB_OBJECT_LIMIT = 100;

/** 탐색기가 처음 그려져 `db.explorer.reveal` 을 등록하기를 기다리는 시간. */
export const REVEAL_WAIT_MS = 3000;

export interface CachedNamesState {
  readonly items: readonly DbCachedObject[];
  readonly truncated: boolean;
  /** 읽는 중(처음 읽기 전도). */
  readonly loading: boolean;
  /** 미신뢰 프로젝트라 Host 가 거절했다. */
  readonly untrusted: boolean;
  readonly error?: string;
  /** 읽기를 마칠 때마다 오른다(늦게 온 결과의 첫 줄 고르기). */
  readonly version: number;
}

const EMPTY: CachedNamesState = {
  items: [],
  truncated: false,
  loading: false,
  untrusted: false,
  version: 0,
};

function untrustedError(e: unknown): boolean {
  if (!(e instanceof RpcCallError) || e.code !== ERROR_CODE.PERMISSION_DENIED) return false;
  const data = e.data as { reason?: unknown } | null | undefined;
  return data?.reason === "untrusted";
}

/** 창의 캐시된 이름. 팝업을 열 때 `refresh` 로 다시 읽는다. */
export class CachedNames {
  private state: CachedNamesState = EMPTY;
  private token = 0;
  private readonly listeners = new Set<() => void>();

  constructor(private readonly read: () => Promise<DbSchemaCached>) {}

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  get = (): CachedNamesState => this.state;

  private set(next: Partial<CachedNamesState>) {
    this.state = { ...this.state, ...next };
    for (const l of this.listeners) l();
  }

  refresh() {
    const token = ++this.token;
    this.set({ loading: true });
    this.read().then(
      (cached) => {
        if (token !== this.token) return;
        this.set({
          items: cached.items,
          truncated: cached.truncated,
          loading: false,
          untrusted: false,
          error: undefined,
          version: this.state.version + 1,
        });
      },
      (e: unknown) => {
        if (token !== this.token) return;
        const untrusted = untrustedError(e);
        this.set({
          items: [],
          truncated: false,
          loading: false,
          untrusted,
          error: untrusted ? undefined : e instanceof Error ? e.message : String(e),
          version: this.state.version + 1,
        });
      },
    );
  }
}

/** 목록의 값(cmdk) — 같은 이름도 연결 · DB · 스키마 · 종류가 다르면 다른 줄이다. */
export function dbObjectValue(item: DbCachedObject): string {
  const { database, schema, name } = item.object;
  return `db:${JSON.stringify([item.connectionId, database, schema, name, item.kind])}`;
}

/** 이름 아래 적는 자리: 연결 · DB · 스키마. */
function placeOf(item: DbCachedObject): string {
  return [item.connectionName, item.object.database, item.object.schema]
    .filter((p): p is string => !!p)
    .join(" · ");
}

/** 질의에 맞는 것(점수순, 같으면 받은 차례). */
export function matchDbObjects(items: readonly DbCachedObject[], query: string) {
  return items
    .map((item, at) => ({
      item,
      at,
      score: Math.max(
        searchFilter(item.object.name, query),
        // 자리(스키마 · DB · 연결)로만 맞으면 이름으로 맞은 것보다 뒤다.
        searchFilter("", query, [
          item.object.schema ?? "",
          item.object.database ?? "",
          item.connectionName,
        ]) * 0.5,
      ),
    }))
    .filter((x) => x.score > 0)
    .sort((a, b) => b.score - a.score || a.at - b.at)
    .map((x) => x.item);
}

/** `registry` 에 `id` 가 등록될 때까지(최대 `ms`) 기다린다. 등록됐으면 참. */
export function untilRegistered(
  registry: ActionRegistry,
  id: string,
  ms: number,
): Promise<boolean> {
  if (registry.isAvailable(id)) return Promise.resolve(true);
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      off();
      resolve(registry.isAvailable(id));
    }, ms);
    const off = registry.subscribe(() => {
      if (!registry.isAvailable(id)) return;
      clearTimeout(timer);
      off();
      resolve(true);
    });
  });
}

interface Deps {
  names: CachedNames;
  api(): DockviewApi | undefined;
}

/** 탐색기를 띄우고 그 객체를 고른다. */
async function revealInExplorer(deps: Deps, registry: ActionRegistry, item: DbCachedObject) {
  const api = deps.api();
  if (api) showPanel(api, "database");
  if (!(await untilRegistered(registry, EXPLORER_ACTIONS.reveal, REVEAL_WAIT_MS))) return;
  registry.run(EXPLORER_ACTIONS.reveal, {
    connectionId: item.connectionId,
    object: item.object,
    kind: item.kind,
  });
}

function isOpenDataKey(e: KeyboardEvent, mac: boolean): boolean {
  if (e.code !== "Enter" && e.code !== "NumpadEnter") return false;
  if (e.isComposing || e.keyCode === 229 || e.altKey || e.shiftKey) return false;
  return mac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
}

function makeResults(deps: Deps) {
  return function DbObjectResults({
    query,
    limit,
    heading,
    onMore,
    choose,
    onResults,
  }: SearchResultsProps) {
    const T = useAppStrings().search;
    const { registry, platform } = useKeymap();
    const state = useSyncExternalStore(deps.names.subscribe, deps.names.get);
    const matched = useMemo(
      () =>
        matchDbObjects(
          state.items.filter((i) => i.kind === "table" || i.kind === "view"),
          query,
        ),
      [state.items, query],
    );
    const max = limit ?? DB_OBJECT_LIMIT;
    const shown = matched.slice(0, max);
    const byValue = useMemo(() => new Map(shown.map((i) => [dbObjectValue(i), i])), [shown]);
    const selected = useCommandState((s) => s.value);
    const current = useRef<DbCachedObject | undefined>(undefined);
    current.current = byValue.get(selected);
    const canOpenData = registry.isAvailable(EXTERNAL_ACTIONS.tableData);

    // 늦게 온 결과를 그렸다 — 팝업이 첫 줄을 고른다.
    useLayoutEffect(() => {
      if (state.version > 0) onResults();
    }, [state.version, onResults]);

    // ⌘↩ · Ctrl+Enter: 고른 줄의 테이블 데이터. cmdk 의 Enter(탐색기에서 고르기)보다 먼저 받는다.
    useEffect(() => {
      const mac = platform === "mac";
      const onKey = (e: KeyboardEvent) => {
        if (!isOpenDataKey(e, mac)) return;
        const item = current.current;
        if (!item) return;
        e.preventDefault();
        e.stopPropagation();
        if (!registry.isAvailable(EXTERNAL_ACTIONS.tableData)) return;
        choose(() =>
          registry.run(EXTERNAL_ACTIONS.tableData, {
            connectionId: item.connectionId,
            object: item.object,
          }),
        );
      };
      window.addEventListener("keydown", onKey, true);
      return () => window.removeEventListener("keydown", onKey, true);
    }, [platform, registry, choose]);

    const notes: ReactNode[] = [];
    if (heading === undefined) {
      if (state.untrusted) notes.push(<Note key="u">{T.db.untrusted}</Note>);
      else if (state.error)
        notes.push(
          <Note key="e" alert>
            {T.failed(state.error)}
          </Note>,
        );
      else if (!state.loading && state.items.length === 0)
        notes.push(<Note key="n">{T.db.none}</Note>);
      else if (state.truncated) notes.push(<Note key="t">{T.db.truncated}</Note>);
    }
    if (shown.length === 0 && notes.length === 0) return null;
    const dataKey = platform === "mac" ? "⌘↩" : "Ctrl+Enter";
    const items = shown.map((item) => {
      const value = dbObjectValue(item);
      return (
        <Command.Item
          key={value}
          value={value}
          data-db-object={item.object.name}
          onSelect={() => choose(() => void revealInExplorer(deps, registry, item))}
          className={SEARCH_ITEM_CLASS}
        >
          <span className="flex min-w-0 flex-1 items-baseline gap-2">
            <span className="flex-none">{item.object.name}</span>
            <span className="min-w-0 truncate text-[11px] text-ui-muted-foreground">
              {placeOf(item)}
            </span>
          </span>
          {canOpenData && value === selected && (
            <kbd className="flex-none font-sans text-[11px] text-ui-muted-foreground">
              {T.db.openData(dataKey)}
            </kbd>
          )}
          <span className="flex-none text-[11px] text-ui-muted-foreground">
            {item.kind === "view" ? T.db.view : T.db.table}
          </span>
        </Command.Item>
      );
    });
    if (heading === undefined)
      return (
        <>
          {items}
          {notes}
        </>
      );
    return (
      <Command.Group heading={heading} className={GROUP_HEADING_CLASS}>
        {items}
        {onMore && matched.length > shown.length && <MoreItem id="dbObjects" onMore={onMore} />}
      </Command.Group>
    );
  };
}

function Note({ children, alert }: { children: ReactNode; alert?: boolean }) {
  return (
    <div
      role={alert ? "alert" : "status"}
      className="truncate px-3 py-1 text-[11px] text-ui-muted-foreground"
    >
      {children}
    </div>
  );
}

/** 결과 묶음(시험이 가짜 원천으로 만든다). */
export function dbObjectSearchSource(deps: Deps): SearchSource {
  return {
    id: "dbObjects",
    Results: makeResults(deps),
    onOpen: () => deps.names.refresh(),
  };
}

/** 프로젝트 창의 「DB 객체」 탭. 창이 사는 동안 하나 둔다. */
export function DbObjectSearch({
  projectId,
  api,
}: {
  projectId: string;
  /** 도킹 영역 — 고르면 DB 탐색기 툴 윈도우를 띄운다. */
  api: DockviewApi | undefined;
}) {
  const { client } = useHost();
  const worktree = useActiveWorktree();
  const live = useRef({ client, projectId, worktree, api });
  live.current = { client, projectId, worktree, api };
  const [source] = useState(() =>
    dbObjectSearchSource({
      names: new CachedNames(() => {
        const { client: c, projectId: id, worktree: wt } = live.current;
        if (!c) return Promise.reject(new Error("Host is not connected"));
        return c.call("db.schema.cached", { projectId: id, worktree: wt ?? null });
      }),
      api: () => live.current.api,
    }),
  );
  useSearchSource(source);
  return null;
}
