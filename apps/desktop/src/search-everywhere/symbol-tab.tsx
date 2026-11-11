// 프로젝트 창이 Search Everywhere 에 더하는 심볼(15-m4-plan M4-16): 「심볼」 탭(심볼로 이동 ⌘⌥O)과 탭이 따로 없는 `classes`
// 모드(클래스로 이동 ⌘O — 심볼 탭을 켠 채 타입 종류만, 종류 표는 symbol-kinds.ts 한 곳), 전체 탭의 심볼 그룹.
//
// - 질의 · 합치기는 symbol-search.ts(인덱스 + 서버가 준비된 언어의 `workspace/symbol`, 200 ms 규칙, 「바쁨」 재시도, 취소된 파일
//   목록의 다시 만들기). 찾는 곳은 파일 트리의 활성 워크트리다(파일 탭과 같다 — `treeScope`).
// - 고르면 위치로 가기(`workspace.navigateTo` — 떠나는 자리가 내비게이션 기록에 든다, M4-09). 루트 밖 결과(서버가 준 라이브러리
//   심볼)는 토큰째 연다 — M4-08 의 토큰 길(외부 소스 탭, 읽기 전용)뿐이고, 그 탭 머리의 정보(경로 · 서버 · 언어)를 먼저 적는다.
// - 서버가 준 이름 · 컨테이너 · 경로는 글자로만 그린다(React 의 글 노드 — HTML 로 해석하지 않는다).
// - 프로젝트 창에만 둔다(파일 창 · 웰컴 · 설정 창에는 탭이 없다). 액션 `search.gotoSymbol` · `search.gotoClass` 도 이 창에만 등록된다.
// SE 팝업은 KeymapProvider 아래(이 컴포넌트 밖)에 그려지므로 결과 컴포넌트는 context 가 아니라 클로저로 이 창의 것을 받는다
// (project-search.tsx 와 같다).

import type { Action } from "@pitwall/keymap";
import { Command } from "cmdk";
import {
  memo,
  type ReactNode,
  type Ref,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";

import type { EditorWorkspace } from "../editor/editor-workspace";
import { treeScope } from "../file-tree/project-view";
import { type AppStrings, useAppStrings } from "../i18n/app-strings";
import { useKeymap, useRegisterActions, useSearchSource } from "../keymap/keymap-provider";
import { navigationHost } from "../lsp/navigation";
import { useHost } from "../rpc/host";
import { MATCH_CLASS } from "../search/match-text";
import { SEARCH_ITEM_CLASS } from "../ui/search-popup";
import {
  GROUP_HEADING_CLASS,
  MoreItem,
  type SearchMode,
  type SearchResultsProps,
  type SearchSource,
} from "./search-everywhere";
import {
  SYMBOL_LIMIT,
  SymbolSearch,
  type SymbolRow,
  type SymbolScope,
  type SymbolSearchDeps,
} from "./symbol-search";

export function symbolSearchActions(
  strings: AppStrings["actions"],
  openSearch: (mode: SearchMode) => void,
): Action[] {
  return [
    {
      id: "search.gotoSymbol",
      label: strings.gotoSymbol,
      keywords: ["Go to Symbol", "Navigate Symbol", "workspace symbol"],
      run: () => openSearch("symbols"),
    },
    {
      id: "search.gotoClass",
      label: strings.gotoClass,
      keywords: ["Go to Class", "Navigate Class", "type"],
      run: () => openSearch("classes"),
    },
  ];
}

interface Deps {
  readonly projectId: string;
  readonly workspace: EditorWorkspace;
  readonly search: SymbolSearch;
}

/** 찾는 곳: 파일 트리의 프로젝트 · 활성 워크트리. */
function scopeOf(projectId: string): SymbolScope {
  return { projectId, worktree: treeScope.getState().worktree ?? null };
}

/** 결과를 연다: 위치로 가기(기록에 넣는다). 루트 밖이면 토큰째(외부 소스 탭), 루트 안이면 찾은 워크트리로. */
function openRow(deps: Deps, row: SymbolRow, scope: SymbolScope | undefined) {
  const worktree = scope?.worktree ?? undefined;
  if (row.token)
    navigationHost()?.rememberToken?.(row.token, {
      path: row.path,
      server: row.server ?? "",
      language: row.language ?? null,
      ...(worktree !== undefined && { worktree }),
    });
  void deps.workspace.navigateTo(
    {
      path: row.path,
      line: row.line,
      column: row.column,
      ...(row.token ? { token: row.token } : worktree !== undefined && { worktree }),
    },
    { record: true },
  );
}

function Marked({ text, positions }: { text: string; positions: readonly number[] }) {
  if (positions.length === 0) return <>{text}</>;
  const hits = new Set(positions);
  const out: ReactNode[] = [];
  let run = "";
  let marked = false;
  const flush = () => {
    if (!run) return;
    out.push(
      marked ? (
        <mark key={out.length} data-match="" className={MATCH_CLASS}>
          {run}
        </mark>
      ) : (
        <span key={out.length}>{run}</span>
      ),
    );
    run = "";
  };
  for (let i = 0; i < text.length; i++) {
    const hit = hits.has(i);
    if (hit !== marked) {
      flush();
      marked = hit;
    }
    run += text[i];
  }
  flush();
  return <>{out}</>;
}

/** 파일 자리: 루트 안이면 상대 경로, 밖이면 경로(또는 URI) 그대로. */
function fileOf(row: SymbolRow): string {
  return row.relative ?? row.path;
}

/** 한 줄. 같은 줄 · 같은 질의면 다시 그리지 않는다(친 글자의 답이 오기 전에 목록 전체가 다시 그려지지 않게). */
const SymbolItem = memo(function SymbolItem({
  row,
  query,
  scope,
  deps,
  choose,
  itemRef,
}: {
  row: SymbolRow;
  /** 이 줄을 낸 질의(`data-symbol-query` — 어느 질의의 결과가 그려졌는지 밖에서 본다). */
  query: string | undefined;
  scope: SymbolScope | undefined;
  deps: Deps;
  choose: SearchResultsProps["choose"];
  /** 목록의 첫 줄에만 — 고른 값을 cmdk 목록에서 읽는 자리(`keepsSelection`). */
  itemRef?: Ref<HTMLDivElement>;
}) {
  const onSelect = () => choose(() => openRow(deps, row, scope));
  const T = useAppStrings().search;
  const file = fileOf(row);
  const name = file.slice(file.lastIndexOf("/") + 1);
  return (
    <Command.Item
      ref={itemRef}
      value={`symbol:${row.key}`}
      onSelect={onSelect}
      className={SEARCH_ITEM_CLASS}
      data-symbol-source={row.source}
      data-symbol-kind={row.kind}
      data-symbol-query={query}
    >
      <span className="flex min-w-0 flex-1 items-baseline gap-2">
        <span className="w-16 flex-none truncate text-[11px] text-ui-muted-foreground">
          {T.symbols.kinds[row.kind]}
        </span>
        <span className="min-w-0 flex-none truncate" data-symbol-name="">
          <Marked text={row.name} positions={row.positions} />
        </span>
        {row.container !== undefined && (
          <span
            className="min-w-0 truncate text-[11px] text-ui-muted-foreground"
            data-symbol-container=""
          >
            {row.container}
          </span>
        )}
      </span>
      <span
        className="max-w-[45%] flex-none truncate text-[11px] text-ui-muted-foreground"
        title={`${row.path}:${row.line}`}
      >
        {name} · {T.symbols.at(row.line)}
      </span>
      {row.token !== undefined && (
        <span className="flex-none text-[11px] text-ui-muted-foreground">{T.symbols.outside}</span>
      )}
    </Command.Item>
  );
});

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

const NO_ROWS: readonly SymbolRow[] = [];

/**
 * 빈 목록에 결과를 처음 그릴 때 먼저 올리는 줄 수 — 목록(높이 360 px · 줄 26 px)에 보이는 줄보다 조금 많다. 나머지는 그 프레임을
 * 그린 뒤에 올린다. 창을 연 뒤 첫 키는 빈 목록에 50줄을 한꺼번에 올려 그 몫(웹뷰 4 – 5 ms)이 이어지는 키(1 – 2 ms)보다 컸다
 * (15-m4-plan §3 ⌘⌥O 첫 키 행).
 */
export const FIRST_ROWS = 16;

/**
 * 같은 질의의 결과가 다시 그려졌을 때(늦은 서버 결과가 위에 붙음 · 「바쁨」 뒤 다시 받음) 지금 고른 것을 그대로 둘지: 사용자가 첫
 * 줄(`firstBefore` — 지난 그림의 첫 줄)에서 옮겨 둔 줄이 새 목록에 남아 있으면 둔다 — 서버는 5 초까지 늦을 수 있어, 화살표로
 * 내려간 뒤 Enter 직전에 붙으면 다른 심볼이 열린다. 첫 줄에 그대로 있었으면 새 첫 줄을 고른다(팝업의 `onResults`).
 */
function keepsSelection(item: HTMLElement | null, firstBefore: string | undefined): boolean {
  const selected = item
    ?.closest("[cmdk-list]")
    ?.querySelector('[cmdk-item][aria-selected="true"]')
    ?.getAttribute("data-value");
  return selected != null && selected !== "" && selected !== firstBefore;
}

function makeSymbolResults(deps: Deps, types: boolean) {
  return function SymbolResults({
    query,
    limit,
    heading,
    onMore,
    choose,
    onResults,
  }: SearchResultsProps) {
    const T = useAppStrings().search;
    const text = query.trim();
    useEffect(() => {
      // 질의가 비면 지난 질의의 재시도 · 도는 요청을 멈춘다.
      if (text) deps.search.request(text, types, scopeOf(deps.projectId));
      else deps.search.stop();
    }, [text]);
    // 팝업이 닫히거나 심볼을 보이지 않는 탭으로 가면 멈춘다.
    useEffect(() => () => deps.search.stop(), []);
    const state = useSyncExternalStore(deps.search.subscribe, deps.search.get);
    const rows = text && state.types === types ? state.rows : NO_ROWS;
    const max = limit ?? SYMBOL_LIMIT;
    // 앞이 빈 목록이었으면(팝업을 열었거나 결과가 없었다) 보이는 줄만 먼저 — 「더 보기」 가 붙는 묶음(SE 전체 탭)에는 하지 않는다.
    const [grown, setGrown] = useState<readonly SymbolRow[] | null>(null);
    const wasEmpty = useRef(true);
    // 한 결과를 부분으로 그리기로 정하면 다 올릴 때까지 지킨다 — 첫 줄 고르기(`onResults`)가 그 프레임 전에 다시 그려도 50줄을 한꺼번에
    // 올리지 않게.
    const partialFor = useRef<readonly SymbolRow[] | null>(null);
    if (
      heading === undefined &&
      rows !== partialFor.current &&
      rows !== grown &&
      wasEmpty.current &&
      rows.length > FIRST_ROWS
    )
      partialFor.current = rows;
    const partial = rows === partialFor.current && rows !== grown;
    const shown = rows.slice(0, partial ? FIRST_ROWS : max);
    useLayoutEffect(() => {
      wasEmpty.current = shown.length === 0;
    });
    useEffect(() => {
      if (!partial) return;
      // 그 프레임을 그린 뒤(rAF 콜백에서 건 작업은 그 프레임의 렌더 갱신 뒤에 돈다).
      let task: ReturnType<typeof setTimeout> | undefined;
      const frame = requestAnimationFrame(() => {
        task = setTimeout(() => setGrown(rows), 0);
      });
      return () => {
        cancelAnimationFrame(frame);
        clearTimeout(task);
      };
    }, [partial, rows]);
    // 결과는 질의보다 늦게 온다(서버 결과는 더 늦게 앞에 더해진다) — 새 질의의 결과를 그리면 팝업이 첫 줄을 고른다. 같은 질의의
    // 결과를 다시 그린 것이면 사용자가 옮겨 둔 고르기를 둔다(`keepsSelection`).
    const firstItem = useRef<HTMLDivElement>(null);
    const drawn = useRef<{ query?: string; types?: boolean; first?: string }>({});
    useLayoutEffect(() => {
      if (rows === NO_ROWS) return;
      const before = drawn.current;
      drawn.current = {
        query: state.query,
        types: state.types,
        first: rows[0] && `symbol:${rows[0].key}`,
      };
      const again = before.query === state.query && before.types === state.types;
      if (again && keepsSelection(firstItem.current, before.first)) return;
      onResults();
    }, [rows, onResults]);
    const items = shown.map((row, i) => (
      <SymbolItem
        key={row.key}
        itemRef={i === 0 ? firstItem : undefined}
        row={row}
        query={state.query}
        scope={state.scope}
        deps={deps}
        choose={choose}
      />
    ));
    const notes: ReactNode[] = [];
    if (text && heading === undefined) {
      if (state.suspended)
        notes.push(
          <Command.Item
            key="rebuild"
            value="symbols:rebuild"
            onSelect={() => void deps.search.rebuild()}
            className={SEARCH_ITEM_CLASS}
            data-symbol-rebuild=""
          >
            <span className="flex-1 truncate text-ui-muted-foreground">{T.symbols.suspended}</span>
            <span className="flex-none">{T.symbols.rebuild}</span>
          </Command.Item>,
        );
      if (state.error !== undefined)
        notes.push(
          <Note key="e" alert>
            {state.busy ? T.symbols.busy : T.failed(state.error)}
          </Note>,
        );
      else if (state.pending && shown.length === 0) notes.push(<Note key="p">{T.searching}</Note>);
      else if (state.lspPending) notes.push(<Note key="l">{T.symbols.waitingServer}</Note>);
    }
    if (items.length === 0 && notes.length === 0) return null;
    if (heading === undefined)
      return (
        <>
          {items}
          {notes}
        </>
      );
    return (
      <Command.Group
        heading={items.length > 0 ? heading : undefined}
        className={GROUP_HEADING_CLASS}
      >
        {items}
        {onMore && rows.length > shown.length && <MoreItem id="symbols" onMore={onMore} />}
      </Command.Group>
    );
  };
}

/** 프로젝트 창의 심볼 검색(⌘⌥O · ⌘O · SE 심볼 탭). 창이 사는 동안 하나 둔다. */
export function SymbolSearchFeature({
  projectId,
  workspace,
}: {
  projectId: string;
  workspace: EditorWorkspace;
}) {
  const { openSearch } = useKeymap();
  const strings = useAppStrings().actions;
  const { client } = useHost();
  const live = useRef({ client, workspace });
  live.current = { client, workspace };
  const [search] = useState(() => {
    const host = () => {
      const c = live.current.client;
      if (!c) throw new Error("Host is not connected");
      return c;
    };
    const deps: SymbolSearchDeps = {
      index: (params, signal) => host().call("index.symbols.search", params, { signal }),
      lsp: (params, signal) => {
        const lsp = live.current.workspace.lsp;
        if (!lsp) return Promise.reject(new Error("no language servers in this window"));
        return lsp.services.call("lsp.workspaceSymbol", params, { signal });
      },
      status: (scope) => host().call("index.status", scope),
      // 파일 목록을 다시 만든다 — 취소된 목록은 다음 파일 검색이 다시 만든다(M4-15 ⓘ). `index.*` 는 만들지 않는다.
      rebuild: (scope) => host().call("search.files", { ...scope, query: "", limit: 1 }),
    };
    return new SymbolSearch(deps);
  });
  useEffect(() => () => search.dispose(), [search]);

  const actions = useMemo(() => symbolSearchActions(strings, openSearch), [strings, openSearch]);
  useRegisterActions(actions);

  const deps = useRef<{ projectId: string; workspace: EditorWorkspace }>({ projectId, workspace });
  deps.current = { projectId, workspace };
  const sources = useMemo(() => {
    const d: Deps = {
      get projectId() {
        return deps.current.projectId;
      },
      get workspace() {
        return deps.current.workspace;
      },
      search,
    };
    return {
      symbols: {
        id: "symbols",
        Results: makeSymbolResults(d, false),
        onOpen: () => search.forget(),
      } satisfies SearchSource,
      classes: {
        id: "classes",
        Results: makeSymbolResults(d, true),
        onOpen: () => search.forget(),
      } satisfies SearchSource,
    };
  }, [search]);
  useSearchSource(sources.symbols);
  useSearchSource(sources.classes);
  return null;
}
