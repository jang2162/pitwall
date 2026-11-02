// 구조 툴 윈도우(15-m4-plan M4-10, 10-ui §2.4 「구조(아웃라인)」): 활성 탭의 심볼 트리(종류 표지 · 이름 · 세부). 트리는 창의
// 아웃라인(outline-service.ts)을 그대로 보이고, 줄은 가상화한다(TanStack Virtual — 1만 심볼에서도 보이는 줄만 그린다).
//
// - 머리 줄: 출처(LSP · 서버 / 인덱스)를 흐리게, 커서 따라가기 · 이름 순 정렬 토글(`aria-pressed`). 토글은 창이 살아 있는 동안만
//   기억한다(프로젝트 상태에 남기지 않는다).
// - 커서 따라가기: 켜져 있으면 탭의 커서를 담은 가장 안쪽 심볼을 고르고 그 조상을 펼쳐 보이게 스크롤한다. 고르기만 하고 포커스는
//   옮기지 않는다.
// - 고르기: 클릭 · `Enter` 는 그 심볼의 이름 자리로 위치로 가기(`navigateTo` — 기록에 넣는다, M4-09). 키: `↑` `↓` 이동, `→` 펼침 ·
//   첫 자식, `←` 접음 · 부모, `Home` · `End`.
// - 이름 · 세부는 서버가 준 글이다 — React 의 글자 노드로만 그린다(HTML 로 해석하지 않는다). 종류는 글자 표지(색만이 아니다)와
//   줄의 접근성 이름(「이름, 종류」)으로 보인다.

import { useVirtualizer } from "@tanstack/react-virtual";
import type { DockviewApi, IDockviewPanelProps } from "dockview-react";
import { ArrowDownAZ, ChevronDown, ChevronRight, Crosshair } from "lucide-react";
import {
  type KeyboardEvent,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { createStore } from "zustand/vanilla";
import { useStore } from "zustand";

import { useEditorWorkspace } from "../editor/editor-workspace";
import {
  ancestors,
  innermostAt,
  type Outline,
  type OutlineSort,
  type OutlineSymbol,
  visibleRows,
} from "./outline";
import { outlineService, type OutlineService } from "./outline-service";
import { goToSymbol, SourceLabel, SymbolLabel } from "./symbol-row";
import { useStructureStrings } from "./strings";

export const ROW_HEIGHT = 22;

/** 창이 살아 있는 동안의 토글(툴 윈도우를 닫았다 열어도 남는다). */
const toggles = createStore<{ follow: boolean; sort: OutlineSort }>(() => ({
  follow: true,
  sort: "position",
}));

/** 아웃라인을 본다. `active` 가 거짓이면(툴 윈도우가 숨겨짐) 읽지 않는다. */
export function useOutline(service: OutlineService, active = true, api?: DockviewApi) {
  useEffect(() => (active ? service.retain(api) : undefined), [service, active, api]);
  return useSyncExternalStore(service.store.subscribe, service.store.getState);
}

/**
 * 툴 윈도우가 보이는가: 그 dock 의 앞 탭이고 dock 이 접혀 있지 않다. 숨긴 툴 윈도우도 도킹 영역에 마운트돼 있고(레이아웃 복원),
 * 숨기기(⌘⇧F12 · 같은 열기 액션)는 dock 을 접기만 한다 — 보이는 동안만 읽는다.
 */
function useVisible(props: IDockviewPanelProps): boolean {
  const { api } = props;
  return useSyncExternalStore(
    (listener) => {
      const subs = [api.onDidVisibilityChange(listener), api.onDidGroupChange(listener)];
      let group = api.group;
      let collapse = group.api.onDidCollapsedChange(listener);
      const regroup = api.onDidGroupChange(() => {
        collapse.dispose();
        group = api.group;
        collapse = group.api.onDidCollapsedChange(listener);
      });
      return () => {
        for (const sub of subs) sub.dispose();
        regroup.dispose();
        collapse.dispose();
      };
    },
    () => api.isVisible && !api.group.api.isCollapsed(),
  );
}

export function StructureToolWindow(props: IDockviewPanelProps) {
  const { api, containerApi } = props;
  const T = useStructureStrings();
  const workspace = useEditorWorkspace();
  const service = outlineService(workspace);
  const state = useOutline(service, useVisible(props), containerApi);
  const follow = useStore(toggles, (s) => s.follow);
  const sort = useStore(toggles, (s) => s.sort);
  const outline = state.status === "ready" ? state.outline : undefined;
  return (
    <div
      data-tool-window={api.id}
      tabIndex={-1}
      className="flex h-full flex-col bg-ui-panel text-ui-foreground outline-none"
    >
      <div className="flex h-[26px] flex-none items-center gap-1 border-b border-ui-border px-2 text-[12px]">
        <span className="min-w-0 flex-1 truncate">
          {outline && <SourceLabel outline={outline} strings={T} />}
        </span>
        <ToggleButton
          pressed={follow}
          label={T.followCursor}
          onClick={() => toggles.setState({ follow: !follow })}
        >
          <Crosshair size={13} aria-hidden />
        </ToggleButton>
        <ToggleButton
          pressed={sort === "name"}
          label={T.sortByName}
          onClick={() => toggles.setState({ sort: sort === "name" ? "position" : "name" })}
        >
          <ArrowDownAZ size={13} aria-hidden />
        </ToggleButton>
      </div>
      {outline && outline.symbols.length > 0 ? (
        <StructureTree
          key={`${state.panelId}:${state.path}`}
          outline={outline}
          cursor={state.cursor}
          follow={follow}
          sort={sort}
          onGo={(symbol) => void goToSymbol(workspace, state, symbol)}
        />
      ) : (
        <p className="p-2 text-[12px] text-ui-muted-foreground" data-structure-empty="">
          {state.status === "none"
            ? T.emptyNone
            : state.status === "loading"
              ? T.emptyLoading
              : state.status === "failed"
                ? T.failed
                : T.emptySymbols}
        </p>
      )}
      {outline?.truncated && (
        <p className="flex-none border-t border-ui-border px-2 py-1 text-[11px] text-ui-muted-foreground">
          {T.truncated}
        </p>
      )}
    </div>
  );
}

function ToggleButton({
  pressed,
  label,
  onClick,
  children,
}: {
  pressed: boolean;
  label: string;
  onClick(): void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-pressed={pressed}
      aria-label={label}
      title={label}
      onClick={onClick}
      className={`flex h-5 w-5 flex-none items-center justify-center rounded-sm hover:bg-ui-hover ${
        pressed ? "bg-ui-active text-ui-foreground" : "text-ui-muted-foreground"
      }`}
    >
      {children}
    </button>
  );
}

function StructureTree({
  outline,
  cursor,
  follow,
  sort,
  onGo,
}: {
  outline: Outline;
  cursor: number | undefined;
  follow: boolean;
  sort: OutlineSort;
  onGo(symbol: OutlineSymbol): void;
}) {
  const T = useStructureStrings();
  const [collapsed, setCollapsed] = useState<ReadonlySet<number>>(new Set());
  const [selectedId, setSelectedId] = useState<number>();
  const scrollRef = useRef<HTMLDivElement>(null);
  const rows = useMemo(() => visibleRows(outline, sort, collapsed), [outline, sort, collapsed]);
  const selected = selectedId === undefined ? -1 : rows.findIndex((r) => r.id === selectedId);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    getItemKey: (index) => rows[index]?.id ?? index,
    overscan: 12,
  });
  const [scrollPending, setScrollPending] = useState(0);
  useLayoutEffect(() => {
    if (scrollPending > 0 && selected >= 0) virtualizer.scrollToIndex(selected, { align: "auto" });
  }, [scrollPending, selected, virtualizer]);

  const select = (id: number | undefined) => {
    if (id === undefined) return;
    setSelectedId(id);
    setScrollPending((n) => n + 1);
  };

  // 커서 따라가기: 커서를 담은 가장 안쪽 심볼을 고르고, 접힌 조상은 펼친다.
  useEffect(() => {
    if (!follow || cursor === undefined) return;
    const id = innermostAt(outline, cursor);
    if (id === undefined) return;
    const up = ancestors(outline, id);
    setCollapsed((prev) =>
      up.some((a) => prev.has(a)) ? new Set([...prev].filter((c) => !up.includes(c))) : prev,
    );
    setSelectedId(id);
    setScrollPending((n) => n + 1);
  }, [follow, cursor, outline]);

  const toggle = (id: number, open?: boolean) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      const close = open === undefined ? !next.has(id) : !open;
      if (close) next.add(id);
      else next.delete(id);
      return next;
    });

  const onKeyDown = (e: KeyboardEvent) => {
    const row = rows[selected];
    const symbol = row && outline.symbols[row.id];
    switch (e.key) {
      case "ArrowDown":
        select(rows[selected < 0 ? 0 : Math.min(rows.length - 1, selected + 1)]?.id);
        break;
      case "ArrowUp":
        select(rows[Math.max(0, selected - 1)]?.id);
        break;
      case "Home":
        select(rows[0]?.id);
        break;
      case "End":
        select(rows.at(-1)?.id);
        break;
      case "ArrowRight":
        if (symbol && symbol.children.length > 0) {
          if (collapsed.has(symbol.id)) toggle(symbol.id, true);
          else select(rows[selected + 1]?.id);
        }
        break;
      case "ArrowLeft":
        if (symbol && symbol.children.length > 0 && !collapsed.has(symbol.id))
          toggle(symbol.id, false);
        else if (symbol?.parent !== undefined) select(symbol.parent);
        break;
      case "Enter":
        if (symbol) onGo(symbol);
        break;
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  };

  return (
    <div
      ref={scrollRef}
      role="tree"
      aria-label={T.tree}
      aria-activedescendant={selected >= 0 ? `structure-row-${rows[selected]!.id}` : undefined}
      tabIndex={0}
      data-keymap-context="structure"
      onKeyDown={onKeyDown}
      className="min-h-0 flex-1 overflow-auto text-[12px] outline-none"
    >
      <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
        {virtualizer.getVirtualItems().map((item) => {
          const row = rows[item.index];
          if (!row) return null;
          const symbol = outline.symbols[row.id]!;
          const open = !collapsed.has(row.id);
          const isSelected = item.index === selected;
          return (
            <div
              key={item.key}
              id={`structure-row-${row.id}`}
              role="treeitem"
              aria-level={row.depth + 1}
              aria-expanded={symbol.children.length > 0 ? open : undefined}
              aria-selected={isSelected}
              aria-label={T.row(symbol.name, T.kinds[symbol.kind])}
              data-structure-row={row.id}
              onClick={() => {
                setSelectedId(row.id);
                onGo(symbol);
              }}
              style={{
                transform: `translateY(${item.start}px)`,
                height: ROW_HEIGHT,
                paddingLeft: 4 + row.depth * 12,
              }}
              className={`absolute top-0 left-0 flex w-full cursor-default items-center gap-1 pr-2 ${
                isSelected ? "bg-ui-active" : "hover:bg-ui-hover"
              }`}
            >
              {symbol.children.length > 0 ? (
                <button
                  type="button"
                  tabIndex={-1}
                  aria-hidden
                  onClick={(e) => {
                    e.stopPropagation();
                    toggle(row.id);
                  }}
                  className="flex h-4 w-3 flex-none items-center justify-center"
                >
                  {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
                </button>
              ) : (
                <span className="w-3 flex-none" />
              )}
              <SymbolLabel symbol={symbol} strings={T} />
            </div>
          );
        })}
      </div>
    </div>
  );
}
