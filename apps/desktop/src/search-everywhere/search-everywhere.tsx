// Search Everywhere(⇧⇧, 10-ui §4.2, 결정 M1-D18): 화면 상단의 검색 팝업 하나에 탭 넷(전체 · 파일 · 액션 · 프로젝트)을 둔다. 액션 찾기
// (⌘⇧A) · 파일로 이동(⌘⇧O) · 프로젝트 스위처(⌘⇧P)는 이 팝업을 그 탭으로 연 것이고, 최근 파일(⌘E)은 탭 없이 최근 파일만 보이는 모드다.
// 창에 cmdk 팝업이 하나뿐이라 둘이 서로 포커스를 뺏지 않는다(KeymapProvider 가 연다).
//
// - 액션 탭은 모든 창에 있다. 파일 · 최근 파일 · 프로젝트는 그 창이 `SearchSource` 로 등록한다(프로젝트 창, src/search-everywhere/
//   project-search.tsx). 액션밖에 없는 창(웰컴 · 설정)에서는 탭 줄을 두지 않는다 — 예전 액션 찾기와 같다.
// - 목록의 거르기 · 순서는 cmdk 가 아니라 결과를 그리는 쪽이 정한다(`shouldFilter={false}`) — 파일 결과는 Host 가 이미 걸러
//   점수순으로 준다. 액션은 액션 찾기와 같은 비교(ui/search-popup.tsx `searchFilter`, 한글 자모 단위)로 거른다.
// - cmdk 는 질의가 바뀐 순간에만 첫 줄을 고른다. 그때는 옛 결과가 보이므로, 늦게 온 결과(파일)를 그리면 결과 쪽이 `onResults` 로
//   알리고 팝업이 그 목록의 첫 줄을 고른다 — 그러지 않으면 순서가 바뀐 새 결과에서 옛 첫 줄이 고른 채로 남아 Enter 가 1위가
//   아닌 파일을 연다.
// - 전체 탭은 그룹마다 `ALL_TAB_LIMIT` 개까지 보이고, 더 있으면 그 탭으로 가는 「더 보기」 줄을 둔다. Tab · ⇧Tab 이 탭을 돈다.
// - 조합을 확정하는 Enter 는 cmdk 가 고르지 않고, 조합을 취소하는 Esc 는 팝업을 닫지 않는다(ui/ime.ts). Tab 도 조합 중이면 탭을
//   돌지 않는다.

import * as Dialog from "@radix-ui/react-dialog";
import { Command } from "cmdk";
import {
  type ComponentType,
  type KeyboardEvent as ReactKeyboardEvent,
  useCallback,
  useMemo,
  useRef,
  useState,
} from "react";

import { type AppStrings, useAppStrings } from "../i18n/app-strings";
import { useI18n } from "../i18n/i18n-provider";
import { useKeymap, useRegistryVersion } from "../keymap/keymap-provider";
import { isComposingKey, keepOpenWhileComposing } from "../ui/ime";
import { SEARCH_ITEM_CLASS, searchFilter } from "../ui/search-popup";

/** 팝업의 탭. `recentFiles` 는 탭 줄 없이 최근 파일만 보인다(⌘E). */
export type SearchMode = "all" | "files" | "actions" | "projects" | "recentFiles";

/** 창이 등록하는 결과 묶음. 액션은 팝업이 직접 그린다. */
export type SearchSourceId = "files" | "projects" | "recentFiles";

/** 탭 줄의 차례. */
export const SEARCH_TABS = ["all", "files", "actions", "projects"] as const;

/** 전체 탭에서 그룹마다 보이는 수. */
export const ALL_TAB_LIMIT = 6;

export interface SearchResultsProps {
  /** 입력 칸의 글자 그대로. */
  readonly query: string;
  /** 전체 탭이면 그룹의 상한. 없으면 그 탭 하나가 목록 전체다. */
  readonly limit?: number;
  /** 전체 탭의 그룹 제목. */
  readonly heading?: string;
  /** 「더 보기」 를 누르면 그 탭으로 간다(전체 탭에서만 있다). */
  readonly onMore?: () => void;
  /** 항목을 골랐다: 팝업을 닫고, 닫히며 포커스가 이전 자리로 돌아간 뒤에 `then` 을 부른다. */
  choose(then: () => void): void;
  /** 질의보다 늦게 온 결과를 그렸다(그린 뒤 layout effect 에서). 팝업이 목록의 첫 줄을 고른다. */
  onResults(): void;
}

export interface SearchSource {
  readonly id: SearchSourceId;
  readonly Results: ComponentType<SearchResultsProps>;
  /**
   * 이 모드로 열 때 처음 고른 상태로 둘 항목의 값(최근 파일이 바로 전 파일을 고른다). 없으면 cmdk 가 첫 항목을 고른다. 처음
   * 그릴 때 정해야 한다 — 그린 뒤에 바꾸면 cmdk 의 첫 항목 고르기가 덮는다.
   */
  readonly initialValue?: () => string | undefined;
  /** 팝업이 열릴 때(어느 탭으로든). 지난번에 받은 결과를 버리는 데 쓴다 — 그사이 파일이 바뀌었을 수 있다. */
  readonly onOpen?: () => void;
}

/** 등록된 결과 묶음으로 열 수 있는 모드. 없는 탭으로 열면 전체 탭이다. */
export function availableMode(mode: SearchMode, sources: readonly SearchSource[]): SearchMode {
  if (mode === "all" || mode === "actions") return mode;
  return sources.some((s) => s.id === mode) ? mode : "all";
}

/** 탭 줄에 보일 탭. 액션 말고 탭이 없으면 빈 목록(탭 줄을 두지 않는다). */
export function visibleTabs(sources: readonly SearchSource[]): SearchMode[] {
  const tabs = SEARCH_TABS.filter(
    (t) => t === "all" || t === "actions" || sources.some((s) => s.id === t),
  );
  return tabs.length > 2 ? tabs : [];
}

export function SearchEverywhere({
  mode,
  onModeChange,
  sources,
}: {
  /** 열린 모드. `undefined` 면 닫혔다. */
  mode: SearchMode | undefined;
  onModeChange(mode: SearchMode | undefined): void;
  sources: readonly SearchSource[];
}) {
  return (
    <Dialog.Root
      open={mode !== undefined}
      onOpenChange={(open) => !open && onModeChange(undefined)}
    >
      <Dialog.Portal>
        <Dialog.Overlay cmdk-overlay="" className="fixed inset-0 bg-black/20" />
        {mode !== undefined && (
          <SearchContent mode={mode} onModeChange={onModeChange} sources={sources} />
        )}
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function modeLabel(T: AppStrings, mode: SearchMode, tabs: readonly SearchMode[]): string {
  if (mode === "recentFiles") return T.search.recentFiles;
  if (mode === "actions" || tabs.length === 0) return T.findAction.label;
  return T.search.label;
}

function placeholder(T: AppStrings, mode: SearchMode, tabs: readonly SearchMode[]): string {
  if (mode === "actions" || tabs.length === 0) return T.findAction.placeholder;
  return T.search.placeholder[mode];
}

function SearchContent({
  mode,
  onModeChange,
  sources,
}: {
  mode: SearchMode;
  onModeChange(mode: SearchMode | undefined): void;
  sources: readonly SearchSource[];
}) {
  const T = useAppStrings();
  const [query, setQuery] = useState("");
  // 고른 항목. 늦게 온 결과에서 첫 줄을 고르려고 cmdk 밖에 둔다(`selectFirst`). 고르기가 바뀌어도 결과는 다시 그리지 않는다(`body`).
  const [selected, setSelected] = useState(
    () =>
      (mode === "all" || mode === "actions"
        ? undefined
        : sources.find((s) => s.id === mode)?.initialValue?.()) ?? "",
  );
  const list = useRef<HTMLDivElement>(null);
  const selectFirst = useCallback(() => {
    const first = list.current?.querySelector('[cmdk-item]:not([aria-disabled="true"])');
    setSelected(first?.getAttribute("data-value") ?? "");
  }, []);
  const tabs = useMemo(() => visibleTabs(sources), [sources]);
  const label = modeLabel(T, mode, tabs);
  const opened = useRef(false);
  if (!opened.current) {
    // 결과를 그리기 전에 버려야 한다 — effect 에서 하면 첫 그림이 지난 결과를 쓴다.
    opened.current = true;
    for (const s of sources) s.onOpen?.();
  }
  const choose = useCallback(
    (then: () => void) => {
      onModeChange(undefined);
      setTimeout(then, 0);
    },
    [onModeChange],
  );

  const onKeyDown = (e: ReactKeyboardEvent) => {
    if (e.key !== "Tab" || tabs.length === 0 || isComposingKey(e.nativeEvent)) return;
    e.preventDefault();
    const at = tabs.indexOf(mode);
    onModeChange(tabs[(at + (e.shiftKey ? -1 : 1) + tabs.length) % tabs.length]);
  };

  // 같은 요소를 돌려주면 React 가 결과를 다시 그리지 않는다 — 화살표로 고르기만 바뀔 때.
  const body = useMemo(() => {
    const common = { query, choose, onResults: selectFirst };
    const source = (id: SearchSourceId) => sources.find((s) => s.id === id);
    if (mode === "all" && tabs.length > 0) {
      const group = (id: SearchSourceId, heading: string) => {
        const s = source(id);
        return (
          s && (
            <s.Results
              key={id}
              {...common}
              limit={ALL_TAB_LIMIT}
              heading={heading}
              onMore={() => onModeChange(id)}
            />
          )
        );
      };
      return (
        <>
          {group("files", T.search.tabs.files)}
          <ActionResults
            {...common}
            limit={ALL_TAB_LIMIT}
            heading={T.search.tabs.actions}
            onMore={() => onModeChange("actions")}
          />
          {group("projects", T.search.tabs.projects)}
        </>
      );
    }
    if (mode === "all" || mode === "actions") return <ActionResults {...common} />;
    const s = source(mode);
    return s && <s.Results {...common} />;
  }, [T, choose, mode, onModeChange, query, selectFirst, sources, tabs]);

  return (
    <Dialog.Content
      cmdk-dialog=""
      data-search-everywhere={mode}
      aria-label={label}
      aria-describedby={undefined}
      onEscapeKeyDown={keepOpenWhileComposing}
      onKeyDown={onKeyDown}
      className="fixed top-[15%] left-1/2 w-[640px] max-w-[90vw] -translate-x-1/2 overflow-hidden rounded-md border border-ui-border bg-ui-panel text-ui-foreground shadow-lg"
    >
      <Dialog.Title className="sr-only">{label}</Dialog.Title>
      <Command label={label} shouldFilter={false} loop value={selected} onValueChange={setSelected}>
        {mode === "recentFiles" && (
          <div className="truncate border-b border-ui-border px-3 py-1 text-center text-[12px] text-ui-muted-foreground">
            {label}
          </div>
        )}
        {tabs.length > 0 && mode !== "recentFiles" && (
          <div role="tablist" className="flex gap-1 border-b border-ui-border px-2 pt-1">
            {tabs.map((tab) => (
              <button
                key={tab}
                type="button"
                role="tab"
                aria-selected={tab === mode}
                tabIndex={-1}
                // 입력 칸의 포커스를 뺏지 않는다.
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => onModeChange(tab)}
                className="rounded-t-sm px-2 py-0.5 text-[12px] text-ui-muted-foreground aria-selected:bg-ui-active aria-selected:text-ui-foreground"
              >
                {T.search.tabs[tab as Exclude<SearchMode, "recentFiles">]}
              </button>
            ))}
          </div>
        )}
        <Command.Input
          autoFocus
          value={query}
          onValueChange={setQuery}
          placeholder={placeholder(T, mode, tabs)}
          className="w-full border-b border-ui-border bg-transparent px-3 py-2 outline-none placeholder:text-ui-muted-foreground"
        />
        <Command.List ref={list} className="max-h-[360px] overflow-y-auto py-1">
          <Command.Empty className="px-3 py-2 text-ui-muted-foreground">
            {mode === "actions" || tabs.length === 0 ? T.findAction.empty : T.search.empty}
          </Command.Empty>
          {body}
        </Command.List>
      </Command>
    </Dialog.Content>
  );
}

/** 그룹 제목. 전체 탭에서만 둔다. */
export const GROUP_HEADING_CLASS =
  "[&_[cmdk-group-heading]]:px-3 [&_[cmdk-group-heading]]:pt-1 [&_[cmdk-group-heading]]:text-[11px] [&_[cmdk-group-heading]]:text-ui-muted-foreground";

/** 전체 탭의 「더 보기」 줄. */
export function MoreItem({ id, onMore }: { id: string; onMore(): void }) {
  const T = useAppStrings().search;
  return (
    <Command.Item value={`more:${id}`} onSelect={onMore} className={SEARCH_ITEM_CLASS}>
      <span className="text-ui-muted-foreground">{T.more}</span>
    </Command.Item>
  );
}

/** 액션 탭: 이 창에서 지금 실행할 수 있는 액션(액션 찾기 자신은 뺀다). */
function ActionResults({ query, limit, heading, onMore, choose }: SearchResultsProps) {
  const { registry, shortcuts, runnable } = useKeymap();
  const T = useAppStrings().findAction;
  const { locales } = useI18n();
  useRegistryVersion(registry);
  const scored = registry
    .list()
    .filter((a) => runnable(a.id) && a.id !== "action.find" && a.id !== "search.everywhere")
    .map((action) => ({
      action,
      score: searchFilter(action.id, query, [action.label, ...(action.keywords ?? [])]),
    }))
    .filter((x) => x.score > 0)
    .sort((a, b) => b.score - a.score || a.action.label.localeCompare(b.action.label, locales[0]));
  const shown = limit === undefined ? scored : scored.slice(0, limit);
  if (shown.length === 0) return null;
  const items = shown.map(({ action }) => {
    const keys = shortcuts(action.id);
    return (
      <Command.Item
        key={action.id}
        value={`action:${action.id}`}
        onSelect={() => choose(() => registry.run(action.id))}
        className={SEARCH_ITEM_CLASS}
      >
        <span className="flex-1 truncate">{action.label}</span>
        {keys.map((k) => (
          <kbd
            key={k.text}
            title={k.reserved ? T.reserved(k.reserved) : undefined}
            className={`font-sans text-[11px] text-ui-muted-foreground ${k.reserved ? "line-through" : ""}`}
          >
            {k.text}
          </kbd>
        ))}
      </Command.Item>
    );
  });
  if (heading === undefined) return <>{items}</>;
  return (
    <Command.Group heading={heading} className={GROUP_HEADING_CLASS}>
      {items}
      {onMore && scored.length > shown.length && <MoreItem id="actions" onMore={onMore} />}
    </Command.Group>
  );
}
