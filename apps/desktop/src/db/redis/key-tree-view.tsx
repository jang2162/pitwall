// 키 트리의 화면(14-m3-plan M3-18): 모델(key-tree.ts)이 편 줄을 TanStack Virtual 로 보이는 것만 그린다. 그룹은 펼침 · 접기와 아래 키
// 수, 키는 타입과 마지막 조각(전체 이름은 툴팁). 마우스(10-ui.md §5.6 「DB」 의 Redis 행): 클릭 고르기, 더블클릭 값 보기, 우클릭
// 메뉴(값 보기 · TTL 수정 · 삭제 · 이름 바꾸기 · 이름 복사). 키: ↑ ↓ 고르기, → 펼침, ← 접음(키면 그 그룹으로), Enter 값 보기.
//
// 스캔 중에는 줄을 다시 펴는 것을 `RELAYOUT_MS` 에 한 번으로 묶는다 — 100만 키 그룹을 펼친 채 묶음마다 다시 펴면 스크롤이 끊긴다.

import * as ContextMenu from "@radix-ui/react-context-menu";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ChevronDown, ChevronRight } from "lucide-react";
import {
  type KeyboardEvent,
  type ReactNode,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { useKeymapPopup } from "../../keymap/keymap-provider";
import { type KeyEntry, type KeyGroup, type KeyRows, type KeyTree } from "./key-tree";
import { useDbRedisStrings } from "./strings";
import { typeName } from "./value-view";

export const ROW_HEIGHT = 22;
/** 스캔 중 줄을 다시 펴는 간격(머리 주석). */
export const RELAYOUT_MS = 200;

export interface KeyMenuActions {
  open(entry: KeyEntry): void;
  editTtl(entry: KeyEntry): void;
  remove(entry: KeyEntry): void;
  rename(entry: KeyEntry): void;
  copyName(entry: KeyEntry): void;
}

/** 트리가 바뀐 것을 따라 다시 그릴 번호. 스캔 중에는 `RELAYOUT_MS` 에 한 번. */
function useTreeVersion(tree: KeyTree, loading: boolean): number {
  const [version, setVersion] = useState(tree.version);
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const update = () => {
      if (!loading) return setVersion(tree.version);
      timer ??= setTimeout(() => {
        timer = undefined;
        setVersion(tree.version);
      }, RELAYOUT_MS);
    };
    const off = tree.subscribe(update);
    setVersion(tree.version);
    return () => {
      off();
      if (timer !== undefined) clearTimeout(timer);
    };
  }, [tree, loading]);
  return version;
}

function MenuItem({ children, onSelect }: { children: ReactNode; onSelect(): void }) {
  return (
    <ContextMenu.Item
      onSelect={onSelect}
      className="flex h-[24px] cursor-default items-center gap-3 px-3 outline-none data-[highlighted]:bg-ui-active"
    >
      {children}
    </ContextMenu.Item>
  );
}

export function KeyTreeView({
  tree,
  loading,
  selected,
  onSelect,
  actions,
  readOnly,
}: {
  tree: KeyTree;
  loading: boolean;
  selected: KeyEntry | undefined;
  onSelect(entry: KeyEntry | undefined): void;
  actions: KeyMenuActions;
  readOnly: boolean;
}) {
  const T = useDbRedisStrings();
  const version = useTreeVersion(tree, loading);
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set());
  const rows: KeyRows = useMemo(
    () => tree.rows(expanded),
    // `version` 은 트리 안이 바뀐 것을 알린다.
    [tree, expanded, version],
  );
  const [cursor, setCursor] = useState(-1);
  const [menuFor, setMenuFor] = useState<KeyEntry>();
  const [menuOpen, setMenuOpen] = useState(false);
  useKeymapPopup(menuOpen);
  const scrollRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: rows.items.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 16,
    initialRect: { width: 300, height: 600 },
  });
  // 고른 키의 줄(고른 키가 다시 펴진 줄 어디에 있는지).
  const selectedRow = useMemo(() => {
    if (!selected) return -1;
    const index = tree.entries.indexOf(selected);
    return index < 0 ? -1 : rows.items.indexOf(index);
  }, [selected, rows, tree]);
  const at = cursor >= 0 && cursor < rows.items.length ? cursor : selectedRow;
  const [scrollTo, setScrollTo] = useState(-1);
  useLayoutEffect(() => {
    if (scrollTo >= 0) virtualizer.scrollToIndex(scrollTo, { align: "auto" });
  }, [scrollTo, virtualizer]);

  const toggle = (group: KeyGroup, open?: boolean) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      const want = open ?? !next.has(group.path);
      if (want) next.add(group.path);
      else next.delete(group.path);
      return next;
    });

  const pick = (row: number) => {
    const item = rows.items[row];
    if (item === undefined) return;
    setCursor(row);
    setScrollTo(row);
    if (typeof item === "number") onSelect(tree.entries[item]);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    const item = rows.items[at];
    switch (e.key) {
      case "ArrowDown":
        pick(Math.min(rows.items.length - 1, at + 1));
        break;
      case "ArrowUp":
        pick(Math.max(0, at - 1));
        break;
      case "ArrowRight":
        if (item !== undefined && typeof item !== "number") toggle(item, true);
        break;
      case "ArrowLeft":
        if (item !== undefined && typeof item !== "number" && expanded.has(item.path))
          toggle(item, false);
        else {
          // 그 줄을 담은 그룹(한 층 위)으로.
          const depth = rows.depths[at] ?? 0;
          for (let i = at - 1; i >= 0; i--)
            if ((rows.depths[i] ?? 0) < depth) {
              pick(i);
              break;
            }
        }
        break;
      case "Enter":
        if (typeof item === "number") actions.open(tree.entries[item]!);
        else if (item !== undefined) toggle(item);
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  return (
    <ContextMenu.Root onOpenChange={setMenuOpen}>
      <ContextMenu.Trigger asChild>
        <div
          ref={scrollRef}
          role="tree"
          aria-label={T.tree.label}
          tabIndex={0}
          data-testid="redis-key-tree"
          onKeyDown={onKeyDown}
          className="min-h-0 flex-1 overflow-auto outline-none focus-visible:ring-1 focus-visible:ring-ui-accent"
        >
          {rows.items.length === 0 && !loading && (
            <p className="p-2 text-ui-muted-foreground">{T.tree.empty}</p>
          )}
          <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
            {virtualizer.getVirtualItems().map((v) => {
              const item = rows.items[v.index]!;
              const depth = rows.depths[v.index] ?? 0;
              const style = {
                position: "absolute" as const,
                top: 0,
                left: 0,
                right: 0,
                height: ROW_HEIGHT,
                transform: `translateY(${v.start}px)`,
                paddingLeft: 4 + depth * 14,
              };
              const current = v.index === at;
              if (typeof item !== "number") {
                const open = expanded.has(item.path);
                return (
                  <div
                    key={`g${item.path}`}
                    role="treeitem"
                    aria-expanded={open}
                    aria-level={depth + 1}
                    aria-selected={current}
                    data-group={item.path}
                    style={style}
                    onClick={() => {
                      setCursor(v.index);
                      toggle(item);
                    }}
                    // 그룹에는 메뉴가 없다 — 메뉴 부품이 열지 않게 기본 동작을 막는다.
                    onContextMenu={(e) => e.preventDefault()}
                    className={`flex cursor-default items-center gap-1 whitespace-nowrap ${current ? "bg-ui-active" : "hover:bg-ui-hover"}`}
                  >
                    {open ? (
                      <ChevronDown size={12} aria-hidden />
                    ) : (
                      <ChevronRight size={12} aria-hidden />
                    )}
                    <span className="truncate">{item.name}</span>
                    <span className="text-[11px] text-ui-muted-foreground">{item.count}</span>
                  </div>
                );
              }
              const entry = tree.entries[item]!;
              const isSelected = entry === selected;
              return (
                <div
                  key={`k${item}`}
                  role="treeitem"
                  aria-level={depth + 1}
                  aria-selected={isSelected}
                  data-key={entry.name}
                  title={entry.name}
                  style={style}
                  onClick={() => pick(v.index)}
                  onDoubleClick={() => actions.open(entry)}
                  onContextMenu={() => {
                    setCursor(v.index);
                    setMenuFor(entry);
                  }}
                  className={`flex cursor-default items-center gap-2 whitespace-nowrap pl-4 ${isSelected ? "bg-ui-active" : current ? "bg-ui-hover" : "hover:bg-ui-hover"}`}
                >
                  <span className="w-12 flex-none text-[11px] text-ui-muted-foreground">
                    {typeName(entry.info.kind, T.type)}
                  </span>
                  <span className={`truncate ${typeof entry.key === "string" ? "" : "font-mono"}`}>
                    {tree.leafLabel(item)}
                  </span>
                  {typeof entry.key !== "string" && (
                    <span className="text-[11px] text-ui-muted-foreground">{T.tree.bytesKey}</span>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      </ContextMenu.Trigger>
      <ContextMenu.Portal>
        <ContextMenu.Content
          data-testid="redis-key-menu"
          className="min-w-[180px] rounded-md border border-ui-border bg-ui-panel py-1 text-ui-foreground shadow-lg"
        >
          {menuFor && (
            <>
              <MenuItem onSelect={() => actions.open(menuFor)}>{T.key.open}</MenuItem>
              {!readOnly && (
                <>
                  <MenuItem onSelect={() => actions.editTtl(menuFor)}>{T.key.editTtl}</MenuItem>
                  <MenuItem onSelect={() => actions.rename(menuFor)}>{T.key.rename}</MenuItem>
                  <MenuItem onSelect={() => actions.remove(menuFor)}>{T.key.delete}</MenuItem>
                </>
              )}
              <ContextMenu.Separator className="my-1 h-px bg-ui-border" />
              <MenuItem onSelect={() => actions.copyName(menuFor)}>{T.key.copyName}</MenuItem>
            </>
          )}
        </ContextMenu.Content>
      </ContextMenu.Portal>
    </ContextMenu.Root>
  );
}
