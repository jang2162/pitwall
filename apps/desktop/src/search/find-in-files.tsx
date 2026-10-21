// 전체에서 찾기(docs/design/12-m1-plan.md M1-20, F3-4)의 창 상태와 연결. 창마다 웹뷰가 따로라 모듈 상태로 둔다(파일 트리와 같다).
//
// - `⌘⇧F`(`search.findInFiles`) · 파일 트리 우클릭 「이 폴더에서 찾기」(`openFindInFiles`) → 팝업(find-popup.tsx). 팝업은 마지막
//   옵션(질의 · 토글 · 범위 · 마스크)을 기억하고 다시 열면 그대로 다시 찾는다.
// - 찾는 곳은 파일 트리의 루트(활성 워크트리 안의 프로젝트 루트, file-tree/project-view.tsx)와 그 워크트리다. 결과를 열 때 그
//   워크트리를 함께 실어 에디터 탭이 그 루트를 접근 범위로 쓴다(M1-09 의 열기 경로).
// - 팝업이 닫히면 그 검색을 멈춘다. 「툴 윈도우에서 열기」 는 도는 검색을 그대로 「검색 결과」 툴 윈도우(search-results.tsx)로 넘기고
//   팝업은 다음에 새 검색을 쓴다. 툴 윈도우는 고정한 결과 하나를 보이고, 다시 고정하면 앞의 것을 멈추고 바꾼다.

import type { RpcClient } from "@pitwall/rpc-client";
import { useEffect, useMemo } from "react";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import type { EditorWorkspace } from "../editor/editor-workspace";
import { treeScope, windowFileTree } from "../file-tree/project-view";
import { separatorOf } from "../file-tree/tree-model";
import { useRegisterActions } from "../keymap/keymap-provider";
import { showPanel } from "../layout/project-layout";
import { useHost } from "../rpc/host";
import { findActions } from "./find-actions";
import { FindPopup } from "./find-popup";
import {
  DEFAULT_OPTIONS,
  type FindOptions,
  type FindRow,
  type FindScope,
  FindSession,
  type SearchPlace,
  type SearchRunner,
} from "./find-session";
import { useFindStrings } from "./strings";

/** 「검색 결과」 툴 윈도우의 id(layout/project-layout.ts 의 PANELS). */
export const SEARCH_RESULTS_TOOL_WINDOW = "searchResults";

interface FindWindowState {
  open: boolean;
  options: FindOptions;
  /** 팝업이 쓰는 검색. 고정하면 툴 윈도우로 가고 새것으로 바뀐다. */
  session: FindSession;
  /** 툴 윈도우에 고정한 검색과 그때의 옵션. `seq` 는 고정할 때마다 오른다. */
  pinned: { session: FindSession; options: FindOptions; seq: number } | undefined;
}

let client: RpcClient | undefined;

/** 이 창의 Host 연결로 `search.text` 를 연다. 연결이 없으면 실패로 끝난다. */
const runSearch: SearchRunner = (params, signal) => {
  if (!client) throw new Error("Host is not connected");
  return client.stream("search.text", params, { signal });
};

function emptyState(): FindWindowState {
  return {
    open: false,
    options: DEFAULT_OPTIONS,
    session: new FindSession(runSearch),
    pinned: undefined,
  };
}

export const findWindow = createStore<FindWindowState>()(emptyState);

/** 시험용: 창의 찾기 상태를 버린다. */
export function resetFindWindow() {
  const { session, pinned } = findWindow.getState();
  session.clear();
  pinned?.session.clear();
  findWindow.setState(emptyState(), true);
}

/** 팝업을 연다. `scope` 를 주면 그 범위로(파일 트리의 「이 폴더에서 찾기」). */
export function openFindInFiles(scope?: FindScope) {
  findWindow.setState((s) => ({
    open: true,
    options: scope ? { ...s.options, scope } : s.options,
  }));
}

/** 파일 트리 우클릭 「이 폴더에서 찾기」: 폴더면 그 폴더, 파일이면 그 파일의 폴더를 범위로 팝업을 연다. */
export function findInFolder(entry: { path: string; dir: boolean }) {
  const sep = separatorOf(entry.path);
  const path = entry.dir ? entry.path : entry.path.slice(0, entry.path.lastIndexOf(sep)) || sep;
  openFindInFiles({ kind: "folder", path });
}

export function closeFindInFiles() {
  const { open, session } = findWindow.getState();
  if (!open) return;
  session.cancel();
  findWindow.setState({ open: false });
}

export function setFindOptions(options: FindOptions) {
  findWindow.setState({ options });
}

/** 지금 찾을 곳. 트리의 루트를 아직 모르면 `undefined`. */
export function searchPlace(): SearchPlace | undefined {
  const { projectId, worktree } = treeScope.getState();
  const root = windowFileTree().getRoot();
  if (projectId === undefined || root === undefined) return undefined;
  return { projectId, worktree, root };
}

/** 팝업의 검색을 툴 윈도우로 넘기고 팝업을 닫는다. 도는 검색은 툴 윈도우에서 이어진다. */
export function pinFindResults(api: Parameters<typeof showPanel>[0] | undefined) {
  const { session, options, pinned } = findWindow.getState();
  pinned?.session.cancel();
  findWindow.setState({
    open: false,
    pinned: { session, options, seq: (pinned?.seq ?? 0) + 1 },
    session: new FindSession(runSearch),
  });
  if (api) showPanel(api, SEARCH_RESULTS_TOOL_WINDOW);
}

/** 결과 한 줄을 그 줄 · 첫 일치 열에서 연다(열은 1부터, UTF-16 단위 — 에디터의 열과 같다). 에디터로 포커스를 옮긴다. */
export function openFindRow(
  workspace: EditorWorkspace,
  row: FindRow,
  worktree: string | undefined,
) {
  void workspace
    .open({
      path: row.file.path,
      worktree,
      line: row.line.line,
      column: (row.line.ranges[0]?.start ?? 0) + 1,
    })
    .then((panel) => {
      // 팝업이 닫히며 포커스를 이전 자리로 돌려놓은 뒤에 옮긴다.
      if (panel) requestAnimationFrame(() => workspace.focus(panel.id));
    });
}

export function useFindWindow<T>(select: (s: FindWindowState) => T): T {
  return useStore(findWindow, select);
}

/** 프로젝트 창의 전체에서 찾기: 액션(⌘⇧F)과 팝업. 창이 사는 동안 하나 둔다. */
export function FindInFiles({ workspace }: { workspace: EditorWorkspace }) {
  const host = useHost().client;
  useEffect(() => {
    client = host;
  }, [host]);
  const T = useFindStrings();
  const actions = useMemo(() => findActions({ strings: T, open: () => openFindInFiles() }), [T]);
  useRegisterActions(actions);
  return <FindPopup workspace={workspace} />;
}
