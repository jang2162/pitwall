// 「검색 결과」 툴 윈도우(10-ui §2.4 하단 툴 윈도우, 12-m1-plan M1-20): 전체에서 찾기 팝업에서 고정한 결과를 파일별 트리로 보인다.
// 행은 TanStack Virtual 로 보이는 것만 그린다(결과는 많아야 1만 줄 + 파일 행). 고정한 검색이 아직 돌면 여기서 이어 받고 멈출 수 있다.
// 언어 서버의 사용처(위치 팝업의 `⌘↩` — 15-m4-plan M4-08, 결정 M4-D22)도 같은 트리로 받는다 — 줄은 위치로 가기로 열고, 루트 밖
// 파일의 줄은 읽기 토큰째 외부 소스 탭으로 연다.
//
// 키: ↑↓ 이동, → 펼침 · ← 접음(줄 행에서는 그 파일로), Enter · 더블클릭은 줄을 그 위치에서 연다(파일 행은 펼치고 접는다).

import type { SearchLine } from "@pitwall/rpc-client";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { IDockviewPanelProps } from "dockview-react";
import { ChevronDown, ChevronRight, File, Square } from "lucide-react";
import {
  type KeyboardEvent,
  type ReactNode,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { useEditorWorkspace } from "../editor/editor-workspace";
import type { UsageFile } from "../lsp/navigation";
import { openFindRow, useFindWindow } from "./find-in-files";
import { FindStatusText, useFindSnapshot } from "./find-popup";
import type { FindSession } from "./find-session";
import { baseName, MatchText } from "./match-text";
import { useFindStrings } from "./strings";

const ROW_HEIGHT = 22;

/** 트리의 파일 하나. 사용처는 루트 밖 파일이면 읽기 토큰을 든다. */
interface TreeFile {
  readonly path: string;
  readonly token?: string;
  readonly lines: readonly SearchLine[];
}

type ResultRow<F extends TreeFile = TreeFile> =
  | { kind: "file"; file: F; collapsed: boolean }
  | { kind: "line"; file: F; line: SearchLine };

/** 파일의 열쇠(펼침 · 선택). 루트 밖 파일은 토큰으로 가른다. */
const fileKey = (file: TreeFile) => file.token ?? file.path;

/** 파일 행 다음에 (펼쳤으면) 그 줄 행들. */
export function resultRows<F extends TreeFile>(
  files: readonly F[],
  collapsed: ReadonlySet<string>,
): ResultRow<F>[] {
  const rows: ResultRow<F>[] = [];
  for (const file of files) {
    const closed = collapsed.has(fileKey(file));
    rows.push({ kind: "file", file, collapsed: closed });
    if (!closed) for (const line of file.lines) rows.push({ kind: "line", file, line });
  }
  return rows;
}

function rowKey(row: ResultRow): string {
  return row.kind === "file" ? fileKey(row.file) : `${fileKey(row.file)}:${row.line.line}`;
}

/** 파일의 폴더(트리 행의 흐린 꼬리). */
function dirName(path: string): string {
  const at = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return at > 0 ? path.slice(0, at) : "";
}

export function SearchResultsToolWindow({ api }: IDockviewPanelProps) {
  const T = useFindStrings();
  const pinned = useFindWindow((s) => s.pinned);
  return (
    <div
      data-tool-window={api.id}
      tabIndex={-1}
      className="flex h-full flex-col bg-ui-panel text-ui-foreground outline-none"
    >
      {pinned?.kind === "find" ? (
        // 고정이 바뀌면 펼침 · 선택을 처음부터.
        <PinnedResults key={pinned.seq} session={pinned.session} query={pinned.options.query} />
      ) : pinned?.kind === "usages" ? (
        <PinnedUsages key={pinned.seq} title={pinned.title} files={pinned.files} />
      ) : (
        <p className="p-2 text-[12px] text-ui-muted-foreground">{T.resultsEmpty}</p>
      )}
    </div>
  );
}

function PinnedResults({ session, query }: { session: FindSession; query: string }) {
  const T = useFindStrings();
  const workspace = useEditorWorkspace();
  const snapshot = useFindSnapshot(session);
  const worktree = snapshot.params?.worktree ?? undefined;
  return (
    <ResultTree
      // 파일 배열은 받는 동안 같은 배열에 쌓이고 청크마다 스냅숏이 바뀐다(find-session.ts) — 스냅숏으로 다시 펼친다.
      files={snapshot.files}
      version={snapshot}
      label={T.resultsTitle(query)}
      status={
        <span className="flex min-w-0 flex-1 text-ui-muted-foreground">
          <FindStatusText snapshot={snapshot} strings={T} />
        </span>
      }
      action={
        snapshot.status === "searching" && (
          <button
            type="button"
            onClick={() => session.cancel()}
            className="flex flex-none items-center gap-1 rounded-sm px-1 text-ui-muted-foreground hover:bg-ui-hover hover:text-ui-foreground"
          >
            <Square size={10} aria-hidden />
            {T.stop}
          </button>
        )
      }
      open={(file, line) => openFindRow(workspace, { file, line }, worktree)}
    />
  );
}

/** 언어 서버의 사용처(M4-08). 줄은 위치로 가기로 연다 — 루트 밖 파일은 토큰째. */
function PinnedUsages({ title, files }: { title: string; files: readonly UsageFile[] }) {
  const workspace = useEditorWorkspace();
  return (
    <ResultTree
      files={files}
      version={files}
      label={title}
      open={(file, line) =>
        void workspace.navigateTo(
          {
            path: file.path,
            ...(file.token !== undefined
              ? { token: file.token }
              : file.worktree !== undefined && { worktree: file.worktree }),
            line: line.line,
            column: (line.ranges[0]?.start ?? 0) + 1,
          },
          { record: true },
        )
      }
    />
  );
}

function ResultTree<F extends TreeFile>({
  files,
  version,
  label,
  status,
  action,
  open,
}: {
  files: readonly F[];
  /** 바뀌면 다시 펼친다(같은 배열에 쌓이는 찾기 결과). */
  version: unknown;
  label: string;
  status?: ReactNode;
  action?: ReactNode;
  open(file: F, line: SearchLine): void;
}) {
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());
  const [selectedKey, setSelectedKey] = useState<string>();
  const scrollRef = useRef<HTMLDivElement>(null);
  // `version` 이 바뀌면 같은 배열이라도 다시 펼친다(찾기 결과는 받는 동안 같은 배열에 쌓인다).
  const rows = useMemo(() => resultRows(files, collapsed), [files, version, collapsed]);
  const selected =
    selectedKey === undefined ? -1 : rows.findIndex((r) => rowKey(r) === selectedKey);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    getItemKey: (index) => (rows[index] ? rowKey(rows[index]) : index),
    overscan: 12,
  });
  const [scrollPending, setScrollPending] = useState(0);
  useLayoutEffect(() => {
    if (scrollPending > 0 && selected >= 0) virtualizer.scrollToIndex(selected, { align: "auto" });
  }, [scrollPending, selected, virtualizer]);

  const select = (index: number) => {
    const row = rows[index];
    if (!row) return;
    setSelectedKey(rowKey(row));
    setScrollPending((n) => n + 1);
  };
  const toggle = (file: F, unfold?: boolean) =>
    setCollapsed((prev) => {
      const key = fileKey(file);
      const next = new Set(prev);
      const close = unfold === undefined ? !next.has(key) : !unfold;
      if (close) next.add(key);
      else next.delete(key);
      return next;
    });
  const activate = (row: ResultRow<F>) => {
    if (row.kind === "file") toggle(row.file);
    else open(row.file, row.line);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    const row = rows[selected];
    switch (e.key) {
      case "ArrowDown":
        select(selected < 0 ? 0 : Math.min(rows.length - 1, selected + 1));
        break;
      case "ArrowUp":
        select(Math.max(0, selected - 1));
        break;
      case "ArrowRight":
        if (row?.kind === "file" && row.collapsed) toggle(row.file, true);
        else select(selected + 1);
        break;
      case "ArrowLeft":
        if (row?.kind === "file" && !row.collapsed) toggle(row.file, false);
        else if (row?.kind === "line")
          select(rows.findIndex((r) => r.kind === "file" && r.file === row.file));
        break;
      case "Enter":
        if (row) activate(row);
        break;
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  };

  return (
    <>
      <div className="flex h-[26px] flex-none items-center gap-2 border-b border-ui-border px-2 text-[12px]">
        <span className="flex-none font-medium">{label}</span>
        {status}
        {action}
      </div>
      <div
        ref={scrollRef}
        role="tree"
        aria-label={label}
        aria-activedescendant={selected >= 0 ? `search-result-${selected}` : undefined}
        tabIndex={0}
        data-keymap-context="searchResults"
        onKeyDown={onKeyDown}
        className="min-h-0 flex-1 overflow-auto text-[12px] outline-none"
      >
        <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
          {virtualizer.getVirtualItems().map((item) => {
            const row = rows[item.index];
            if (!row) return null;
            const isSelected = item.index === selected;
            return (
              <div
                key={item.key}
                id={`search-result-${item.index}`}
                role="treeitem"
                aria-level={row.kind === "file" ? 1 : 2}
                aria-expanded={row.kind === "file" ? !row.collapsed : undefined}
                aria-selected={isSelected}
                data-search-result={rowKey(row)}
                onClick={() => {
                  setSelectedKey(rowKey(row));
                  if (row.kind === "file") toggle(row.file);
                }}
                onDoubleClick={() => row.kind === "line" && activate(row)}
                style={{ transform: `translateY(${item.start}px)`, height: ROW_HEIGHT }}
                className={`absolute top-0 left-0 flex w-full cursor-default items-center gap-1 pr-2 ${
                  isSelected ? "bg-ui-active" : "hover:bg-ui-hover"
                } ${row.kind === "file" ? "pl-1" : "pl-8"}`}
              >
                {row.kind === "file" ? (
                  <>
                    {row.collapsed ? (
                      <ChevronRight size={12} aria-hidden className="flex-none" />
                    ) : (
                      <ChevronDown size={12} aria-hidden className="flex-none" />
                    )}
                    <File size={13} aria-hidden className="flex-none" />
                    <span className="flex-none">{baseName(row.file.path)}</span>
                    <span
                      className="min-w-0 truncate text-ui-muted-foreground"
                      title={row.file.path}
                    >
                      {dirName(row.file.path)}
                    </span>
                    <span className="ml-auto flex-none text-[11px] text-ui-muted-foreground">
                      {row.file.lines.length}
                    </span>
                  </>
                ) : (
                  <>
                    <span className="w-10 flex-none text-right text-[11px] text-ui-muted-foreground">
                      {row.line.line}
                    </span>
                    <MatchText line={row.line} />
                  </>
                )}
              </div>
            );
          })}
        </div>
      </div>
    </>
  );
}
