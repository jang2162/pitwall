// 프로젝트 툴 윈도우의 파일 트리(F3-1 · F3-6 · F11-8 의 트리 부분, 10-ui §5.4). 상태는 tree-model.ts, Host 연결은 project-view.tsx.
//
// - 행은 TanStack Virtual 로 화면에 보이는 것만 그린다(행 높이 고정). 펼친 행이 5만 개여도 DOM 은 수십 행이다.
// - 클릭 → 고르고 파일이면 미리보기 탭(포커스는 트리에 남는다), 더블클릭 · Enter → 고정 탭(포커스는 에디터로), 폴더는 펼치고 접는다.
//   키보드는 IntelliJ 와 같다: ↑↓ 이동, → 펼침/첫 자식, ← 접음/부모, Home/End, PageUp/PageDown.
// - 글자를 치면 speed search: 위에 뜨는 입력 칸에 이어 치고(두 번째 글자부터는 입력 칸이 받아 IME 조합도 된다), 보이는 행 중
//   이름에 그 글자가 든 것으로 간다. ↑↓ 는 앞뒤 일치, Enter 는 그 행을 연다, Esc 는 닫는다.
// - 우클릭 메뉴: 새 파일 · 새 폴더 · 이름 바꾸기 · 삭제(M1-21, file-ops.ts), 경로 복사 · 상대 경로 복사 · Finder 에서 보기 · 터미널에서
//   열기 · 외부 도구로 열기 · 이 폴더에서 찾기(src/search). 외부 도구는 Host 의 `openWith.run` 이 띄운다(open-with/open-with.tsx).
// - 행을 끌어 폴더(또는 파일 — 그 부모)에 놓으면 옮기고, ⌥ 를 누른 채 놓으면 복사한다. Finder 에서의 드롭은 툴 윈도우가
//   데스크톱의 드롭 알림으로 받는다(tree-drop.ts).
// - 루트 밖을 가리키는 심링크는 「루트 밖을 가리킴」 을 달고 펼치거나 열지 않는다(결정 M1-D8).

import type { Platform } from "@pitwall/keymap";
import * as ContextMenu from "@radix-ui/react-context-menu";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { IDockviewPanelProps } from "dockview-react";
import {
  ChevronDown,
  ChevronRight,
  Eye,
  EyeOff,
  File,
  FileSymlink,
  Folder,
  FolderOpen,
  FolderSymlink,
  LocateFixed,
} from "lucide-react";
import {
  type DragEvent,
  type KeyboardEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { useStore } from "zustand";

import type { FsCopied, FsCreated, FsMoved, FsTrashed } from "@pitwall/rpc-client";

import { useEditorWorkspace } from "../editor/editor-workspace";
import { useAppStrings } from "../i18n/app-strings";
import { useKeymap, useRegisterActions, useRegistryVersion } from "../keymap/keymap-provider";
import { showPanel } from "../layout/project-layout";
import { useProjectStrings } from "../layout/strings";
import { useNotify } from "../notifications/notifications";
import { REVEAL_TOOLS, useOpenWith } from "../open-with/open-with";
import { findInFolder } from "../search/find-in-files";
import { useFindStrings } from "../search/strings";
import { useHost } from "../rpc/host";
import { windowTerminals } from "../terminal/terminal-tabs";
import { isComposingKey } from "../ui/ime";
import { useFileDialogs } from "./file-dialogs";
import { type FileOps, fileOps, fileOpsActions } from "./file-ops";
import { revealRequests, treeScope, windowFileTree } from "./project-view";
import { type FileTreeStrings, useFileTreeStrings } from "./strings";
import { dropFolderAt, folderOf, tauriDropNative, TREE_DRAG_TYPE } from "./tree-drop";
import {
  type FileTreeModel,
  isWithin,
  separatorOf,
  type TreeEntry,
  type TreeRow,
} from "./tree-model";

/** 행 높이(px). 가상화가 이 값으로 위치를 셈한다 — 행의 CSS 높이와 같아야 한다. */
export const ROW_HEIGHT = 22;
const INDENT = 12;

function useModelVersion(model: FileTreeModel): number {
  return useSyncExternalStore(
    useCallback((listener) => model.subscribe(listener), [model]),
    () => model.getVersion(),
  );
}

/** 이름 안에 `query` 가 든 곳(대소문자 무시). 없으면 -1. */
function matchAt(name: string, query: string): number {
  return name.toLowerCase().indexOf(query.toLowerCase());
}

function rowDomId(index: number) {
  return `file-tree-row-${index}`;
}

function relativePath(path: string, root: string | undefined): string {
  if (root === undefined || !isWithin(path, root)) return path;
  if (path === root) return ".";
  const sep = separatorOf(root);
  return path.slice(root.endsWith(sep) ? root.length : root.length + 1);
}

export interface FileTreeHandlers {
  /** 파일을 연다. `preview` 면 미리보기 탭. */
  open(path: string, preview: boolean): void;
  copy(text: string): void;
  /** 없으면 메뉴에서 뺀다. */
  reveal?(path: string): void;
  openWith?(path: string): void;
  /** 「이 폴더에서 찾기」(src/search). 파일이면 그 폴더에서 찾는다. 없으면 메뉴에서 뺀다. */
  findInFolder?(entry: { path: string; dir: boolean }): void;
  /** 파일 조작(M1-21). 없으면 메뉴에서 빼고 드래그를 받지 않는다. */
  ops?: Pick<FileOps, "create" | "rename" | "remove" | "move">;
  /** 「터미널에서 열기」. 폴더 경로를 받는다. */
  openInTerminal?(dir: string): void;
}

function TreeRowView({
  row,
  index,
  selected,
  query,
  strings,
  start,
  dropTarget,
  draggable,
  onClick,
  onDoubleClick,
  onToggle,
  onContextMenu,
}: {
  row: TreeRow;
  index: number;
  selected: boolean;
  query: string | undefined;
  strings: FileTreeStrings;
  start: number;
  /** 끄는 항목을 놓으면 이 폴더로 들어간다. */
  dropTarget: boolean;
  draggable: boolean;
  onClick(row: TreeRow): void;
  onDoubleClick(row: TreeRow): void;
  onToggle(row: TreeRow): void;
  onContextMenu(row: TreeRow): void;
}) {
  const { entry, depth, expanded } = row;
  const Icon = entry.outside
    ? entry.dir
      ? FolderSymlink
      : FileSymlink
    : entry.dir
      ? expanded
        ? FolderOpen
        : entry.symlink
          ? FolderSymlink
          : Folder
      : entry.symlink
        ? FileSymlink
        : File;
  const at = query ? matchAt(entry.name, query) : -1;
  const name: ReactNode =
    at < 0 ? (
      entry.name
    ) : (
      <>
        {entry.name.slice(0, at)}
        <mark className="bg-ui-accent text-ui-accent-foreground">
          {entry.name.slice(at, at + query!.length)}
        </mark>
        {entry.name.slice(at + query!.length)}
      </>
    );
  const muted = entry.ignored || entry.dim || entry.outside;
  return (
    <div
      id={rowDomId(index)}
      role="treeitem"
      aria-level={depth + 1}
      aria-expanded={entry.dir ? expanded : undefined}
      aria-selected={selected}
      data-path={entry.path}
      data-ignored={entry.ignored ? "" : undefined}
      data-dim={entry.dim ? "" : undefined}
      data-outside={entry.outside ? "" : undefined}
      data-drop-target={dropTarget ? "" : undefined}
      title={entry.outside ? strings.outside : undefined}
      draggable={draggable}
      onDragStart={(e) => {
        e.dataTransfer.setData(TREE_DRAG_TYPE, JSON.stringify([entry.path]));
        e.dataTransfer.setData("text/plain", entry.path);
        e.dataTransfer.effectAllowed = "copyMove";
      }}
      onClick={() => onClick(row)}
      onDoubleClick={() => onDoubleClick(row)}
      onContextMenu={() => onContextMenu(row)}
      style={{
        height: ROW_HEIGHT,
        transform: `translateY(${start}px)`,
        paddingLeft: depth * INDENT + 4,
      }}
      className={`absolute top-0 left-0 flex w-full cursor-default items-center gap-1 pr-2 whitespace-nowrap select-none ${
        selected ? "bg-ui-active" : "hover:bg-ui-hover"
      } ${muted ? "text-ui-muted-foreground" : ""} ${
        dropTarget ? "outline outline-1 -outline-offset-1 outline-ui-accent" : ""
      }`}
    >
      <span
        className="flex w-4 flex-none items-center justify-center"
        onClick={(e) => {
          if (!entry.dir) return;
          e.stopPropagation();
          onToggle(row);
        }}
      >
        {entry.dir &&
          (expanded ? (
            <ChevronDown size={12} aria-hidden />
          ) : (
            <ChevronRight size={12} aria-hidden />
          ))}
      </span>
      <Icon size={14} aria-hidden className="flex-none" />
      <span className={`min-w-0 truncate ${entry.ignored ? "italic" : ""}`}>{name}</span>
      {entry.outside && <span className="truncate text-[11px]">{strings.outside}</span>}
      {row.loading && <span className="truncate text-[11px]">{strings.loading}</span>}
      {row.error && (
        <span role="alert" className="truncate text-[11px] text-ui-accent">
          {strings.failed(row.error)}
        </span>
      )}
    </div>
  );
}

function MenuItem({
  onSelect,
  disabled,
  children,
}: {
  onSelect(): void;
  disabled?: boolean;
  children: ReactNode;
}) {
  return (
    <ContextMenu.Item
      onSelect={onSelect}
      disabled={disabled}
      className="flex h-[24px] cursor-default items-center gap-3 px-3 outline-none data-[disabled]:text-ui-muted-foreground data-[highlighted]:bg-ui-active"
    >
      {children}
    </ContextMenu.Item>
  );
}

/**
 * 트리 화면. `model` 의 행을 가상화해 그리고 키보드 · 마우스 · speed search · 우클릭 메뉴를 받는다. `scrollTo` 가 바뀌면(번호가
 * 오르면) 고른 행을 화면 안으로 굴리고 트리에 포커스를 준다(「프로젝트 뷰에서 선택」).
 */
export function FileTree({
  model,
  handlers,
  platform,
  focusRequest = 0,
  className = "",
}: {
  model: FileTreeModel;
  handlers: FileTreeHandlers;
  /** 「Finder 에서 보기」 의 이름. */
  platform: Platform;
  /** 오를 때마다 고른 행을 화면 안으로 굴리고 트리에 포커스를 준다. */
  focusRequest?: number;
  className?: string;
}) {
  const strings = useFileTreeStrings();
  const project = useProjectStrings().editor;
  const openWith = useAppStrings().openWith;
  const findStrings = useFindStrings();
  useModelVersion(model);
  const rows = model.rows();
  const selected = model.selected();
  const selectedIndex = useMemo(
    () => (selected === undefined ? -1 : rows.findIndex((r) => r.entry.path === selected)),
    [rows, selected],
  );
  const scrollRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState<string>();
  const [menuRow, setMenuRow] = useState<TreeRow>();
  /** 끄는 항목을 놓을 폴더(그 행을 테두리로 보인다). */
  const [dropTarget, setDropTarget] = useState<string>();
  /** 고른 행을 다음 그림 뒤에 화면 안으로 굴린다. */
  const [scrollPending, setScrollPending] = useState(0);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    getItemKey: (index) => rows[index]?.entry.path ?? index,
    overscan: 12,
  });

  // 요청 하나에 한 번만 굴린다 — 위에서 폴더가 펼쳐져 고른 행의 번호가 바뀌어도 사람이 굴린 자리를 빼앗지 않게.
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

  const selectIndex = (index: number) => {
    const row = rows[Math.max(0, Math.min(rows.length - 1, index))];
    if (!row) return;
    model.select(row.entry.path);
    setScrollPending((n) => n + 1);
  };

  /** 행을 연다: 폴더는 펼치고 접고, 파일은 탭으로. 루트 밖 링크는 아무것도 하지 않는다. */
  const activate = (row: TreeRow, preview: boolean) => {
    const { entry } = row;
    if (entry.outside) return;
    if (entry.dir) void model.toggle(entry.path);
    else handlers.open(entry.path, preview);
  };

  const matches = useMemo(() => {
    if (!query) return [];
    const out: number[] = [];
    rows.forEach((r, i) => {
      if (matchAt(r.entry.name, query) >= 0) out.push(i);
    });
    return out;
  }, [rows, query]);

  /** 지금 행에서(포함) `step` 방향으로 가장 가까운 일치로 간다. */
  const jumpToMatch = (from: number, step: 1 | -1, list = matches) => {
    if (list.length === 0) return;
    const ordered = step > 0 ? list : [...list].reverse();
    const next = ordered.find((i) => (step > 0 ? i >= from : i <= from)) ?? ordered[0]!;
    selectIndex(next);
  };

  const updateQuery = (next: string) => {
    setQuery(next);
    const list: number[] = [];
    rows.forEach((r, i) => {
      if (matchAt(r.entry.name, next) >= 0) list.push(i);
    });
    jumpToMatch(Math.max(0, selectedIndex), 1, list);
  };

  const closeSearch = (focusTree: boolean) => {
    setQuery(undefined);
    if (focusTree) scrollRef.current?.focus({ preventScroll: true });
  };

  const searching = query !== undefined;
  useEffect(() => {
    if (searching) searchRef.current?.focus();
  }, [searching]);

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
        if (!row?.entry.dir) break;
        if (!row.expanded) void model.expand(row.entry.path);
        else if (rows[selectedIndex + 1]?.depth === row.depth + 1) selectIndex(selectedIndex + 1);
        break;
      case "ArrowLeft": {
        if (!row) break;
        if (row.entry.dir && row.expanded) {
          model.collapse(row.entry.path);
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
        if (row) activate(row, false);
        break;
      default:
        handled = false;
        if (e.key.length === 1 && e.key !== " " && !e.metaKey && !e.ctrlKey && !e.altKey) {
          handled = true;
          updateQuery(e.key);
        }
    }
    if (handled) {
      e.preventDefault();
      e.stopPropagation();
    }
  };

  const onSearchKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (isComposingKey(e.nativeEvent)) return;
    let handled = true;
    switch (e.key) {
      case "ArrowDown":
        jumpToMatch(selectedIndex + 1, 1);
        break;
      case "ArrowUp":
        jumpToMatch(selectedIndex - 1, -1);
        break;
      case "Enter": {
        const row = rows[selectedIndex];
        closeSearch(true);
        if (row) activate(row, false);
        break;
      }
      case "Escape":
        closeSearch(true);
        break;
      case "Backspace":
        if (query === "" || query?.length === 1) {
          closeSearch(true);
          break;
        }
        handled = false;
        break;
      default:
        handled = false;
    }
    if (handled) {
      e.preventDefault();
      e.stopPropagation();
    }
  };

  const root = model.getRoot();
  const target = menuRow?.entry;

  /** 이 요소(행 또는 빈 자리) 위에 놓으면 들어갈 폴더. */
  const folderAt = (element: EventTarget | null): string | undefined => {
    if (root === undefined) return undefined;
    const path = (element as Element | null)
      ?.closest?.("[role=treeitem]")
      ?.getAttribute("data-path");
    const entry = path ? rows.find((r) => r.entry.path === path)?.entry : undefined;
    return entry ? folderOf(entry, root) : root;
  };
  const dragging = (e: DragEvent<HTMLElement>) =>
    handlers.ops !== undefined && e.dataTransfer.types.includes(TREE_DRAG_TYPE);
  const menuFolder = (entry: TreeEntry | undefined) =>
    entry && root !== undefined ? folderOf(entry, root) : root;

  return (
    <ContextMenu.Root>
      <div className={`relative flex min-h-0 flex-1 flex-col ${className}`}>
        {query !== undefined && (
          <div className="absolute top-0 right-2 left-2 z-10 flex items-center gap-1 rounded-sm border border-ui-border bg-ui-panel px-1 shadow">
            <input
              ref={searchRef}
              aria-label={strings.search}
              value={query}
              onChange={(e) => updateQuery(e.target.value)}
              onKeyDown={onSearchKey}
              onBlur={() => closeSearch(false)}
              className={`min-w-0 flex-1 bg-transparent text-[12px] outline-none ${
                query && matches.length === 0 ? "text-ui-accent" : ""
              }`}
            />
            {query && matches.length === 0 && (
              <span className="text-[11px] text-ui-muted-foreground">{strings.searchNone}</span>
            )}
          </div>
        )}
        <ContextMenu.Trigger asChild>
          <div
            ref={scrollRef}
            role="tree"
            aria-label={strings.label}
            aria-activedescendant={selectedIndex >= 0 ? rowDomId(selectedIndex) : undefined}
            tabIndex={0}
            data-keymap-context="fileTree"
            onKeyDown={onTreeKey}
            onContextMenu={(e) => {
              // 빈 자리를 누르면 루트가 대상이다.
              if (e.target === scrollRef.current || e.target === e.currentTarget.firstChild)
                setMenuRow(rows[0]);
            }}
            onDragOver={(e) => {
              if (!dragging(e)) return;
              e.preventDefault();
              e.dataTransfer.dropEffect = e.altKey ? "copy" : "move";
              setDropTarget(folderAt(e.target));
            }}
            onDragLeave={(e) => {
              if (!e.currentTarget.contains(e.relatedTarget as Node | null))
                setDropTarget(undefined);
            }}
            onDragEnd={() => setDropTarget(undefined)}
            onDrop={(e) => {
              if (!dragging(e)) return;
              e.preventDefault();
              setDropTarget(undefined);
              const dir = folderAt(e.target);
              let paths: unknown;
              try {
                paths = JSON.parse(e.dataTransfer.getData(TREE_DRAG_TYPE));
              } catch {
                return;
              }
              if (dir === undefined || !Array.isArray(paths)) return;
              void handlers.ops?.move(
                paths.filter((p): p is string => typeof p === "string"),
                dir,
                e.altKey,
              );
            }}
            className="min-h-0 flex-1 overflow-auto text-[13px] outline-none"
          >
            {root === undefined ? (
              <p className="p-2 text-ui-muted-foreground">{strings.noRoot}</p>
            ) : (
              <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
                {virtualizer.getVirtualItems().map((item) => {
                  const row = rows[item.index];
                  if (!row) return null;
                  return (
                    <TreeRowView
                      key={item.key}
                      row={row}
                      index={item.index}
                      selected={item.index === selectedIndex}
                      query={query}
                      strings={strings}
                      start={item.start}
                      dropTarget={row.entry.path === dropTarget}
                      draggable={handlers.ops !== undefined && row.depth > 0}
                      onClick={(r) => {
                        model.select(r.entry.path);
                        if (!r.entry.dir && !r.entry.outside) handlers.open(r.entry.path, true);
                      }}
                      onDoubleClick={(r) => activate(r, false)}
                      onToggle={(r) => void model.toggle(r.entry.path)}
                      onContextMenu={(r) => {
                        model.select(r.entry.path);
                        setMenuRow(r);
                      }}
                    />
                  );
                })}
              </div>
            )}
          </div>
        </ContextMenu.Trigger>
      </div>
      <ContextMenu.Portal>
        <ContextMenu.Content
          onCloseAutoFocus={(e) => {
            e.preventDefault();
            scrollRef.current?.focus({ preventScroll: true });
          }}
          className="min-w-[200px] rounded-md border border-ui-border bg-ui-panel py-1 text-ui-foreground shadow-lg"
        >
          {handlers.ops && (
            <>
              <MenuItem
                disabled={menuFolder(target) === undefined}
                onSelect={() => {
                  const dir = menuFolder(target);
                  if (dir !== undefined) void handlers.ops?.create(dir, "file");
                }}
              >
                {strings.ops.menu.newFile}
              </MenuItem>
              <MenuItem
                disabled={menuFolder(target) === undefined}
                onSelect={() => {
                  const dir = menuFolder(target);
                  if (dir !== undefined) void handlers.ops?.create(dir, "dir");
                }}
              >
                {strings.ops.menu.newFolder}
              </MenuItem>
              <MenuItem
                disabled={!target || target.path === root}
                onSelect={() => target && void handlers.ops?.rename(target.path)}
              >
                {strings.ops.menu.rename}
              </MenuItem>
              <MenuItem
                disabled={!target || target.path === root}
                onSelect={() => target && void handlers.ops?.remove([target.path])}
              >
                {strings.ops.menu.delete}
              </MenuItem>
              <ContextMenu.Separator className="my-1 h-px bg-ui-border" />
            </>
          )}
          <MenuItem disabled={!target} onSelect={() => target && handlers.copy(target.path)}>
            {project.actions.copyPath}
          </MenuItem>
          <MenuItem
            disabled={!target}
            onSelect={() => target && handlers.copy(relativePath(target.path, root))}
          >
            {strings.copyRelativePath}
          </MenuItem>
          {handlers.reveal && (
            <MenuItem
              disabled={!target || target.outside}
              onSelect={() => target && handlers.reveal?.(target.path)}
            >
              {project.actions.reveal(platform)}
            </MenuItem>
          )}
          {handlers.openInTerminal && (
            <MenuItem
              disabled={!target || target.outside || menuFolder(target) === undefined}
              onSelect={() => {
                const dir = menuFolder(target);
                if (dir !== undefined) handlers.openInTerminal?.(dir);
              }}
            >
              {strings.ops.menu.openInTerminal}
            </MenuItem>
          )}
          {handlers.openWith && (
            <MenuItem
              disabled={!target || target.outside}
              onSelect={() => target && handlers.openWith?.(target.path)}
            >
              {openWith.action}
            </MenuItem>
          )}
          {handlers.findInFolder && (
            <MenuItem
              disabled={!target || target.outside}
              onSelect={() => target && handlers.findInFolder?.(target)}
            >
              {findStrings.inFolder}
            </MenuItem>
          )}
        </ContextMenu.Content>
      </ContextMenu.Portal>
    </ContextMenu.Root>
  );
}

/** 툴 윈도우 머리의 버튼(「열린 파일 선택」 · 무시된 파일 숨기기/보이기). */
function TreeToolbar({ model }: { model: FileTreeModel }) {
  const strings = useFileTreeStrings();
  const { registry } = useKeymap();
  useRegistryVersion(registry);
  useModelVersion(model);
  const hidden = model.ignoredMode() === "hide";
  const selectIn = registry.get("fileTree.selectIn");
  const button =
    "flex size-5 items-center justify-center rounded-sm text-ui-muted-foreground hover:bg-ui-hover hover:text-ui-foreground disabled:opacity-40";
  return (
    <div className="flex h-[26px] flex-none items-center justify-end gap-1 border-b border-ui-border px-1">
      <button
        type="button"
        title={strings.actions.selectIn}
        aria-label={strings.actions.selectIn}
        disabled={!selectIn}
        onClick={() => registry.run("fileTree.selectIn")}
        className={button}
      >
        <LocateFixed size={14} aria-hidden />
      </button>
      <button
        type="button"
        title={hidden ? strings.showIgnored : strings.hideIgnored}
        aria-label={hidden ? strings.showIgnored : strings.hideIgnored}
        aria-pressed={hidden}
        onClick={() => model.setIgnoredMode(hidden ? "dim" : "hide")}
        className={button}
      >
        {hidden ? <EyeOff size={14} aria-hidden /> : <Eye size={14} aria-hidden />}
      </button>
    </div>
  );
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** 트리 범위로 Host 의 파일 조작을 부른다(file-ops.ts 의 `rpc`). */
function hostOps() {
  const scope = () => {
    const { client, projectId, worktree } = treeScope.getState();
    if (!client || projectId === undefined) throw new Error("Host is not connected");
    return { client, base: { projectId, worktree: worktree ?? null } };
  };
  return {
    create: (path: string, kind: "file" | "dir"): Promise<FsCreated> => {
      const { client, base } = scope();
      return client.call("fs.create", { ...base, path, kind });
    },
    rename: (from: string, to: string): Promise<FsMoved> => {
      const { client, base } = scope();
      return client.call("fs.rename", { ...base, from, to });
    },
    copy: (from: string, to: string): Promise<FsCopied> => {
      const { client, base } = scope();
      return client.call("fs.copy", { ...base, from, to });
    },
    trash: (paths: string[], permanent: boolean): Promise<FsTrashed> => {
      const { client, base } = scope();
      return client.call("fs.trash", { ...base, paths, permanent });
    },
  };
}

/**
 * 프로젝트 툴 윈도우(project-window.tsx 의 `TOOL_WINDOWS`). 창의 트리 모델(project-view.tsx)을 그리고, 파일은 창의 에디터
 * 작업 공간으로 열고(탭이 그 워크트리를 기억한다), 「프로젝트 뷰에서 선택」 요청을 받는다. 파일 조작(M1-21)의 액션 · 대화상자 ·
 * Finder 드롭도 여기서 잇는다.
 */
export function FileTreeToolWindow({ api }: IDockviewPanelProps) {
  const model = windowFileTree();
  const workspace = useEditorWorkspace();
  const notify = useNotify();
  const editorStrings = useProjectStrings().editor;
  const strings = useFileTreeStrings();
  const { platform } = useKeymap();
  const { client } = useHost();
  const dialogs = useFileDialogs();
  const latest = useRef({ notify, editorStrings, strings, dialogs });
  latest.current = { notify, editorStrings, strings, dialogs };

  const { run, openPopup, popup } = useOpenWith({
    target: () => undefined,
    onError: (message) => {
      if (message) latest.current.notify({ level: "error", title: message });
    },
  });

  const reveal = useStore(revealRequests);
  const [focusRequest, setFocusRequest] = useState(0);
  useEffect(() => {
    const path = reveal.path;
    if (path === undefined) return;
    revealRequests.setState({ path: undefined });
    void model.reveal(path).then((found) => {
      if (found) setFocusRequest((n) => n + 1);
    });
  }, [reveal, model]);

  const ops = useMemo(
    () =>
      fileOps({
        rpc: hostOps(),
        importDrop: (dropId, dir) => {
          const { projectId, worktree } = treeScope.getState();
          if (projectId === undefined) return Promise.reject(new Error("no project"));
          return tauriDropNative.importDrop({
            dropId,
            projectId,
            worktree,
            dir,
            title: latest.current.strings.ops.drop.task,
          });
        },
        workspace,
        model,
        strings: strings.ops,
        notify: (input) => latest.current.notify(input),
        askName: (q) => latest.current.dialogs.askName(q),
        confirm: (q) => latest.current.dialogs.confirm(q),
        open: (path) => {
          const { worktree } = treeScope.getState();
          void workspace.open({ path, worktree }).then((panel) => {
            if (panel) requestAnimationFrame(() => workspace.view(panel.id)?.focus());
          });
        },
      }),
    [workspace, model, strings],
  );

  const openInTerminal = useCallback(
    (dir: string) => {
      const dock = workspace.api();
      if (dock) showPanel(dock, "terminal");
      const tabs = windowTerminals();
      tabs.open({ cwd: dir, worktree: treeScope.getState().worktree });
      requestAnimationFrame(() => tabs.active()?.focus());
    },
    [workspace],
  );

  const actions = useMemo(
    () =>
      fileOpsActions({
        strings: strings.actions,
        model,
        ops,
        canOpenTerminal: () => client !== undefined,
        openInTerminal,
      }),
    [strings, model, ops, client, openInTerminal],
  );
  useRegisterActions(actions);

  // Finder 에서 트리 위로 드롭하면 그 폴더(빈 자리면 루트)로 복사한다. 트리 밖 드롭(터미널 등)은 다른 곳이 받는다.
  const containerRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    tauriDropNative
      .listen((drop) => {
        const tree = containerRef.current?.querySelector<HTMLElement>("[role=tree]");
        const root = model.getRoot();
        if (!tree || root === undefined) return;
        const scale = window.devicePixelRatio || 1;
        const dir = dropFolderAt(
          tree,
          { x: drop.x / scale, y: drop.y / scale },
          root,
          (path) => model.rows().find((r) => r.entry.path === path)?.entry,
        );
        if (dir !== undefined) void ops.importDrop(drop.id, dir);
      })
      .then((off) => {
        if (disposed) off();
        else unlisten = off;
      })
      .catch(() => {
        // Tauri 밖(시험 · 웹): 드롭을 받지 않는다.
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [model, ops]);

  const handlers = useMemo((): FileTreeHandlers => {
    const target = (file: string) => {
      const { projectId, name, worktree } = treeScope.getState();
      if (projectId === undefined || name === undefined) return undefined;
      return { projectId, name, worktree, file };
    };
    return {
      open: (path, preview) => {
        const { worktree } = treeScope.getState();
        void workspace.open({ path, worktree, preview }).then((panel) => {
          // 고정 탭으로 열었으면 에디터로 간다. 미리보기는 트리에 남아 계속 고를 수 있게 한다.
          if (!preview && panel) requestAnimationFrame(() => workspace.view(panel.id)?.focus());
        });
      },
      copy: (text) =>
        void navigator.clipboard.writeText(text).catch((e: unknown) =>
          latest.current.notify({
            level: "error",
            title: latest.current.editorStrings.copyFailed,
            body: errorText(e),
          }),
        ),
      reveal: (path) => {
        const t = target(path);
        if (t) void run(t, REVEAL_TOOLS[platform]);
      },
      openWith: (path) => {
        const t = target(path);
        if (t) openPopup(t);
      },
      findInFolder,
      ops,
      openInTerminal,
    };
  }, [model, workspace, run, openPopup, platform, ops, openInTerminal]);

  return (
    <div
      ref={containerRef}
      data-tool-window={api.id}
      tabIndex={-1}
      className="flex h-full flex-col bg-ui-panel text-ui-foreground outline-none"
    >
      <TreeToolbar model={model} />
      <FileTree model={model} handlers={handlers} platform={platform} focusRequest={focusRequest} />
      {popup}
      {dialogs.element}
    </div>
  );
}
