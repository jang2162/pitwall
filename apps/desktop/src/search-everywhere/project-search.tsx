// 프로젝트 창이 Search Everywhere 에 더하는 것: 파일 탭(⌘⇧O 파일로 이동), 최근 파일(⌘E), 프로젝트 탭(⌘⇧P 프로젝트 스위처)과 그 액션.
//
// - 파일: `search.files`(crates/pw-fs/src/index, 결정 M1-D5)를 창의 활성 워크트리로 찾는다(파일 트리의 루트와 같다,
//   file-tree/project-view.tsx `treeScope`). 요청은 한 번에 하나만 간다(file-search.ts). 이 창에서 최근에 본 파일(recent-files.ts)이
//   질의에 맞으면 맨 위에 둔다. 빈 질의는 최근 파일만 보인다. `이름:줄[:열]` 이면 그 줄 · 열로 연다(file-query.ts).
// - 열기는 에디터의 열기 경로(editor-workspace.tsx `open`)로, 찾은 워크트리를 함께 싣는다(M1-09).
// - 프로젝트: `project.list` 를 웰컴 창과 같은 순서(즐겨찾기 → 최근에 연 것 → 이름)로. 고르면 네이티브 `open_project_window` 가 그
//   창을 열거나, 이미 열려 있으면 앞으로 가져온다(src-tauri/src/windows.rs `open_project`). 이 창의 프로젝트는 팝업만 닫는다.
// SE 팝업은 KeymapProvider 아래(이 컴포넌트 밖)에 그려지므로 결과 컴포넌트는 context 가 아니라 클로저로 이 창의 것을 받는다.

import type { Action } from "@pitwall/keymap";
import type { FileMatch } from "@pitwall/rpc-client";
import type { DockviewApi } from "dockview-react";
import { Command } from "cmdk";
import {
  memo,
  type ReactNode,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { useStore } from "zustand";

import type { EditorWorkspace } from "../editor/editor-workspace";
import { treeScope, windowFileTree } from "../file-tree/project-view";
import { type AppStrings, useAppStrings } from "../i18n/app-strings";
import { useKeymap, useRegisterActions, useSearchSource } from "../keymap/keymap-provider";
import { useHost, useRpcQuery } from "../rpc/host";
import { MATCH_CLASS } from "../search/match-text";
import { SEARCH_ITEM_CLASS, searchFilter } from "../ui/search-popup";
import { sortProjects } from "../welcome/projects";
import { matchRecent, parseFileQuery } from "./file-query";
import { FILE_LIMIT, FileSearch, type Scope } from "./file-search";
import { createRecentFiles, type RecentFiles } from "./recent-files";
import {
  GROUP_HEADING_CLASS,
  MoreItem,
  type SearchMode,
  type SearchResultsProps,
  type SearchSource,
} from "./search-everywhere";

export function projectSearchActions({
  strings,
  openSearch,
  canSwitchProjects,
}: {
  strings: AppStrings["actions"];
  openSearch(mode: SearchMode): void;
  /** 프로젝트 창을 열 수 있는가(네이티브가 없으면 프로젝트 탭이 없다). */
  canSwitchProjects: boolean;
}): Action[] {
  return [
    {
      id: "search.gotoFile",
      label: strings.gotoFile,
      keywords: ["Go to File", "Navigate File"],
      run: () => openSearch("files"),
    },
    {
      id: "search.recentFiles",
      label: strings.recentFiles,
      keywords: ["Recent Files"],
      run: () => openSearch("recentFiles"),
    },
    ...(canSwitchProjects
      ? [
          {
            id: "project.switcher",
            label: strings.projectSwitcher,
            keywords: ["Switch Project", "Open Recent Project"],
            run: () => openSearch("projects"),
          } satisfies Action,
        ]
      : []),
  ];
}

/** 파일 결과 한 줄. */
interface FileRow {
  readonly path: string;
  /** 루트 기준 상대 경로(`/` 로 가른다). 루트 밖이면 경로 그대로. */
  readonly relative: string;
  readonly positions: readonly number[];
  readonly worktree?: string;
  readonly recent: boolean;
}

/** `path` 가 `root` 아래면 상대 경로(`/`). 아니면 그대로. */
function relativeTo(root: string | undefined, path: string): string {
  if (!root) return path;
  const sep = root.includes("\\") && !root.includes("/") ? "\\" : "/";
  const base = root.endsWith(sep) ? root : root + sep;
  return path.startsWith(base) ? path.slice(base.length).split(sep).join("/") : path;
}

/** 글자 위치(UTF-16)들을 칠한다. `offset` 은 `text` 가 상대 경로에서 시작하는 위치다. */
function Highlighted({
  text,
  positions,
  offset = 0,
}: {
  text: string;
  positions: ReadonlySet<number>;
  offset?: number;
}) {
  const out: ReactNode[] = [];
  let run = "";
  let marked = false;
  let i = 0;
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
  for (const ch of text) {
    const hit = positions.has(offset + i);
    if (hit !== marked) {
      flush();
      marked = hit;
    }
    run += ch;
    i += ch.length;
  }
  flush();
  return <>{out}</>;
}

/** 한 줄. 같은 줄이면 다시 그리지 않는다(키 입력마다 목록 전체가 다시 그려지지 않게). */
const FileItem = memo(function FileItem({
  row,
  line,
  column,
  deps,
  choose,
}: {
  row: FileRow;
  line?: number;
  column?: number;
  deps: Deps;
  choose: SearchResultsProps["choose"];
}) {
  const T = useAppStrings().search;
  const onSelect = () => choose(() => openFile(deps, row, line, column));
  const nameAt = row.relative.lastIndexOf("/") + 1;
  const positions = new Set(row.positions);
  return (
    <Command.Item
      value={`file:${row.path}`}
      onSelect={onSelect}
      className={SEARCH_ITEM_CLASS}
      data-path={row.path}
    >
      <span className="flex min-w-0 flex-1 items-baseline gap-2">
        <span className="flex-none">
          <Highlighted text={row.relative.slice(nameAt)} positions={positions} offset={nameAt} />
        </span>
        {nameAt > 0 && (
          <span className="min-w-0 truncate text-[11px] text-ui-muted-foreground" title={row.path}>
            <Highlighted text={row.relative.slice(0, nameAt - 1)} positions={positions} />
          </span>
        )}
      </span>
      {line !== undefined && (
        <span className="flex-none text-[11px] text-ui-muted-foreground">{T.atLine(line)}</span>
      )}
      {row.recent && (
        <span className="flex-none text-[11px] text-ui-muted-foreground">{T.recentBadge}</span>
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

function Group({ heading, children }: { heading: string | undefined; children: ReactNode }) {
  if (heading === undefined) return <>{children}</>;
  return (
    <Command.Group heading={heading} className={GROUP_HEADING_CLASS}>
      {children}
    </Command.Group>
  );
}

interface Deps {
  projectId: string;
  workspace: EditorWorkspace;
  recent: RecentFiles;
  files: FileSearch;
  openProjectWindow?: (projectId: string) => Promise<void>;
}

/** 찾는 곳: 파일 트리의 프로젝트 · 활성 워크트리. */
function scopeOf(projectId: string): Scope {
  return { projectId, worktree: treeScope.getState().worktree ?? null };
}

/** 파일을 연다. 줄을 주면 그 줄 · 열로. 에디터로 포커스를 옮긴다. */
function openFile(deps: Deps, row: FileRow, line?: number, column?: number) {
  const worktree = row.worktree ?? treeScope.getState().worktree;
  void deps.workspace
    .open({ path: row.path, worktree, ...(line !== undefined && { line, column: column ?? 1 }) })
    .then((panel) => {
      if (panel) requestAnimationFrame(() => deps.workspace.focus(panel.id));
    });
}

/** 최근 파일 중 질의에 맞는 것(최근 순). */
function recentRows(
  deps: Deps,
  list: readonly { path: string; worktree?: string }[],
  text: string,
) {
  const root = windowFileTree().getRoot();
  const rows: FileRow[] = [];
  for (const f of list) {
    const relative = relativeTo(root, f.path);
    const positions = matchRecent(relative, text);
    if (positions)
      rows.push({ path: f.path, relative, positions, worktree: f.worktree, recent: true });
  }
  return rows;
}

const NO_FILES: readonly FileMatch[] = [];

/** 최근 파일 중 질의에 맞는 것(일치 위치는 Host 결과에 있으면 그것)을 먼저, 그다음 Host 결과. */
function fileRows(
  deps: Deps,
  recentList: readonly { path: string; worktree?: string }[],
  server: readonly FileMatch[],
  text: string,
): FileRow[] {
  const byPath = new Map(server.map((f) => [f.path, f]));
  const lifted = recentRows(deps, recentList, text).map((r) => {
    const hit = byPath.get(r.path);
    return hit ? { ...r, relative: hit.relative, positions: hit.positions } : r;
  });
  const seen = new Set(lifted.map((r) => r.path));
  return [
    ...lifted,
    ...server
      .filter((f) => !seen.has(f.path))
      .map((f) => ({ path: f.path, relative: f.relative, positions: f.positions, recent: false })),
  ];
}

function makeFileResults(deps: Deps) {
  return function FileResults({ query, limit, heading, onMore, choose }: SearchResultsProps) {
    const T = useAppStrings().search;
    const { text, line, column } = parseFileQuery(query);
    const trimmed = text.trim();
    useEffect(() => {
      if (trimmed) deps.files.request(text, scopeOf(deps.projectId));
    }, [text, trimmed]);
    const state = useSyncExternalStore(deps.files.subscribe, deps.files.get);
    const recentList = useStore(deps.recent.store, (s) => s.files);
    const server = trimmed ? state.files : NO_FILES;
    const rows = useMemo(
      () => fileRows(deps, recentList, server, text),
      [recentList, server, text],
    );
    const max = limit ?? FILE_LIMIT;
    const shown = rows.slice(0, max);
    const items = shown.map((row) => (
      <FileItem key={row.path} row={row} line={line} column={column} deps={deps} choose={choose} />
    ));
    const notes: ReactNode[] = [];
    if (trimmed && heading === undefined) {
      if (state.error)
        notes.push(
          <Note key="e" alert>
            {state.busy ? T.busy : T.failed(state.error)}
          </Note>,
        );
      else if (state.pending && shown.length === 0) notes.push(<Note key="p">{T.searching}</Note>);
      else if (state.query === text && state.matched > state.files.length)
        notes.push(<Note key="t">{T.truncated(state.files.length, state.matched)}</Note>);
    }
    if (!trimmed && shown.length === 0 && heading === undefined)
      notes.push(<Note key="r">{T.noRecentFiles}</Note>);
    if (items.length === 0 && notes.length === 0) return null;
    return (
      <Group heading={items.length > 0 ? heading : undefined}>
        {items}
        {onMore && rows.length > shown.length && <MoreItem id="files" onMore={onMore} />}
        {notes}
      </Group>
    );
  };
}

function makeRecentResults(deps: Deps) {
  return function RecentResults({ query, choose }: SearchResultsProps) {
    const T = useAppStrings().search;
    const list = useStore(deps.recent.store, (s) => s.files);
    const rows = recentRows(deps, list, query.trim());
    if (rows.length === 0) return query.trim() === "" ? <Note>{T.noRecentFiles}</Note> : null;
    return (
      <>
        {rows.map((row) => (
          <FileItem key={row.path} row={{ ...row, recent: false }} deps={deps} choose={choose} />
        ))}
      </>
    );
  };
}

function makeProjectResults(deps: Deps) {
  return function ProjectResults({ query, limit, heading, onMore, choose }: SearchResultsProps) {
    const T = useAppStrings().search;
    const projects = useRpcQuery("project.list", null);
    const scored = sortProjects(projects.data ?? [])
      .map((p) => ({ p, score: searchFilter(p.name, query, p.roots.slice(0, 1)) }))
      .filter((x) => x.score > 0)
      .sort((a, b) => b.score - a.score);
    const shown = limit === undefined ? scored : scored.slice(0, limit);
    const open = (id: string) => {
      if (id === deps.projectId || !deps.openProjectWindow) return;
      deps
        .openProjectWindow(id)
        .catch((e: unknown) => console.warn("[search] project not opened", id, e));
    };
    if (projects.error && heading === undefined)
      return <Note alert>{T.failed(String(projects.error))}</Note>;
    if (shown.length === 0) return null;
    return (
      <Group heading={heading}>
        {shown.map(({ p }) => (
          <Command.Item
            key={p.id}
            value={`project:${p.id}`}
            onSelect={() => choose(() => open(p.id))}
            className={SEARCH_ITEM_CLASS}
          >
            <span className="flex min-w-0 flex-1 items-baseline gap-2">
              <span className="flex-none">{p.name}</span>
              <span
                className="min-w-0 truncate text-[11px] text-ui-muted-foreground"
                title={p.roots[0]}
              >
                {p.roots[0]}
              </span>
            </span>
            {p.id === deps.projectId && (
              <span className="flex-none text-[11px] text-ui-muted-foreground">
                {T.currentProject}
              </span>
            )}
          </Command.Item>
        ))}
        {onMore && scored.length > shown.length && <MoreItem id="projects" onMore={onMore} />}
      </Group>
    );
  };
}

/** 프로젝트 창의 파일로 이동 · 최근 파일 · 프로젝트 스위처. 창이 사는 동안 하나 둔다. */
export function ProjectSearch({
  projectId,
  workspace,
  api,
  openProjectWindow,
}: {
  projectId: string;
  workspace: EditorWorkspace;
  /** 도킹 영역. 최근 파일은 그 에디터 탭의 활성화로 센다. */
  api: DockviewApi | undefined;
  /** 다른 프로젝트 창을 열거나 앞으로 가져온다. 없으면 프로젝트 탭이 없다. */
  openProjectWindow?: (projectId: string) => Promise<void>;
}) {
  const { openSearch } = useKeymap();
  const strings = useAppStrings().actions;
  const { client } = useHost();
  const clientRef = useRef(client);
  clientRef.current = client;
  const [recent] = useState(createRecentFiles);
  const [files] = useState(
    () =>
      new FileSearch((params) => {
        const c = clientRef.current;
        if (!c) return Promise.reject(new Error("Host is not connected"));
        return c.call("search.files", params);
      }),
  );
  useEffect(() => () => files.dispose(), [files]);
  useEffect(() => (api ? recent.track(api) : undefined), [api, recent]);

  const actions = useMemo(
    () =>
      projectSearchActions({
        strings,
        openSearch,
        canSwitchProjects: openProjectWindow !== undefined,
      }),
    [strings, openSearch, openProjectWindow],
  );
  useRegisterActions(actions);

  const deps = useRef<Deps>(undefined as never);
  deps.current = { projectId, workspace, recent, files, openProjectWindow };
  const sources = useMemo(() => {
    const live: Deps = {
      get projectId() {
        return deps.current.projectId;
      },
      get workspace() {
        return deps.current.workspace;
      },
      recent,
      files,
      get openProjectWindow() {
        return deps.current.openProjectWindow;
      },
    };
    return {
      files: {
        id: "files",
        Results: makeFileResults(live),
        onOpen: () => files.forget(),
      } satisfies SearchSource,
      recentFiles: {
        id: "recentFiles",
        Results: makeRecentResults(live),
        // 바로 전 파일을 고른 채로 연다 — Enter 한 번으로 앞뒤 파일을 오간다(IntelliJ 와 같다).
        initialValue: () => {
          const previous = recent.list()[1];
          return previous && `file:${previous.path}`;
        },
      } satisfies SearchSource,
      projects: { id: "projects", Results: makeProjectResults(live) } satisfies SearchSource,
    };
  }, [recent, files]);
  useSearchSource(sources.files);
  useSearchSource(sources.recentFiles);
  useSearchSource(openProjectWindow ? sources.projects : undefined);
  return null;
}
