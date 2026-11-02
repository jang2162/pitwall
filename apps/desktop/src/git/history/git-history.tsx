// 파일 히스토리를 여는 액션(⌃⌥H — 10-ui.md 「기본 단축키」 의 Git 표, 13-m2-plan M2-18). 다른 행 · 메뉴는 id 로 부른다.
//
//   registry.run("git.showFileHistory")                    지금 파일: 프로젝트 트리에 포커스가 있으면 고른 파일, 아니면 활성
//                                                          에디터 탭의 파일
//   registry.run("git.showFileHistory", { path, worktree? }) 그 파일(절대 경로). 트리 · 탭 우클릭, 거터 우클릭(M2-17)이 쓴다
//
// 경로는 그 워크트리의 저장소 꼭대기(`git.repo`) 기준으로 바꿔 파일 히스토리 탭(params.git 의 `history`)을 연다 — 같은 파일이면 그
// 탭을 앞으로. 파일이 없거나(열린 탭 없음 · git 문서 탭) 폴더 · 저장소 밖이면 열지 않고 경고 알림, 저장소가 없으면 오류 알림이다.
// 따로 된 저장소(서브모듈 · 중첩 저장소) 안의 파일도 열지 않는다 — 그 파일의 커밋은 이 저장소의 이력에 없다. 판정은 트리가 이미
// 읽은 행으로 한다(`nestedRepository` — 위 폴더에 `.git` 이 있는가). 트리가 그 폴더를 펼친 적이 없으면 탭을 열고, Host 가
// 서브모듈이면 거절해(`git.fileHistory` 의 `data.submodule`) 탭이 그 까닭을 보인다.
// 키는 늘 받는다(호스트가 붙어 있으면 켜져 있다) — 메뉴가 인자를 주고 부를 때 포커스가 메뉴에 있어 지금 파일을 판정할 수 없다.

import type { Action } from "@pitwall/keymap";
import { useMemo, useRef } from "react";

import { isFileTab, tabParams } from "../../editor/editor-tabs";
import type { EditorWorkspace } from "../../editor/editor-workspace";
import type { FileTreeModel } from "../../file-tree/tree-model";
import { treeScope, windowFileTree } from "../../file-tree/project-view";
import { useRegisterActions } from "../../keymap/keymap-provider";
import { useNotify } from "../../notifications/notifications";
import { activeWorktree } from "../../open-with/active-worktree";
import { openGitDocument } from "../documents/git-documents";
import { gitDocumentSource } from "../documents/source";
import type { GitDocumentSpec } from "../documents/spec";
import { type GitHistoryStrings, useGitHistoryStrings } from "./strings";

/** 히스토리를 볼 파일. `dir` 은 알 때만(트리). */
export interface HistoryTarget {
  /** 절대 경로. */
  path: string;
  worktree: string | undefined;
  dir?: boolean;
}

const slashes = (path: string) => path.replace(/\\/g, "/");

/** `path` 의 저장소 꼭대기 `top` 기준 경로(`/`). 꼭대기 밖이거나 꼭대기 자신이면 없다. */
export function repoRelative(top: string, path: string): string | undefined {
  const root = slashes(top).replace(/\/+$/, "");
  const file = slashes(path);
  if (!file.startsWith(`${root}/`)) return undefined;
  const rel = file.slice(root.length + 1);
  return rel === "" ? undefined : rel;
}

const SEPARATOR = /[\\/]/;

/**
 * `path` 를 담은 따로 된 저장소(서브모듈 · 중첩 저장소)의 폴더 — 트리 루트 아래의 위 폴더 중 `.git` 이 든 가장 가까운 것을 트리 루트
 * 기준 경로로. 트리가 읽은 행만 본다(펼친 적 없는 폴더는 모른다 — 없다고 답한다).
 */
export function nestedRepository(tree: FileTreeModel, path: string): string | undefined {
  const root = tree.getRoot();
  if (root === undefined || !path.startsWith(root)) return undefined;
  // `/repo2/x` 는 `/repo` 아래가 아니다.
  if (!SEPARATOR.test(path.charAt(root.length)) && !SEPARATOR.test(root.charAt(root.length - 1)))
    return undefined;
  const known = new Set(tree.rows().map((r) => r.entry.path));
  const parts = path.slice(root.length).split(SEPARATOR).filter(Boolean);
  const sep = root.includes("\\") && !root.includes("/") ? "\\" : "/";
  for (let end = parts.length - 1; end >= 1; end--) {
    const dir = `${root.replace(/[\\/]+$/, "")}${sep}${parts.slice(0, end).join(sep)}`;
    if (known.has(`${dir}${sep}.git`)) return parts.slice(0, end).join("/");
  }
  return undefined;
}

/** 인자 없이 부른 때의 파일: 트리에 포커스가 있으면 고른 항목, 아니면 활성 에디터 탭(git 문서 · DB · 외부 소스 탭은 파일이 아니다). */
function currentTarget(workspace: EditorWorkspace): HistoryTarget | undefined {
  const focused = document.activeElement
    ?.closest("[data-tool-window]")
    ?.getAttribute("data-tool-window");
  if (focused === "project") {
    const tree = windowFileTree();
    const path = tree.selected();
    if (path === undefined) return undefined;
    const entry = tree.rows().find((r) => r.entry.path === path)?.entry;
    return { path, worktree: treeScope.getState().worktree, dir: entry?.dir };
  }
  const tab = workspace.activeTab();
  if (!tab || !isFileTab(tab)) return undefined;
  const { path, worktree } = tabParams(tab);
  return { path, worktree };
}

function argsTarget(args: unknown): HistoryTarget | undefined {
  if (typeof args !== "object" || args === null) return undefined;
  const { path, worktree } = args as { path?: unknown; worktree?: unknown };
  if (typeof path !== "string" || path === "") return undefined;
  return { path, worktree: typeof worktree === "string" ? worktree : activeWorktree() };
}

/** `git.showFileHistory`. `current` · `workTree` · `open` · `notify` 는 시험이 가짜를 넘긴다. */
export function fileHistoryAction(deps: {
  strings: () => GitHistoryStrings;
  ready(): boolean;
  current(): HistoryTarget | undefined;
  /** 따로 된 저장소 안이면 그 폴더(`nestedRepository`). */
  nested(path: string): string | undefined;
  /** 그 워크트리의 저장소 꼭대기. 저장소가 없으면 던진다. */
  workTree(worktree: string | undefined): Promise<string>;
  open(spec: GitDocumentSpec, title: string, worktree: string | undefined): void;
  notify(level: "error" | "warning", title: string, body?: string): void;
}): Action {
  return {
    id: "git.showFileHistory",
    label: deps.strings().actions.show,
    keywords: ["Show History", "File History", "History"],
    enabled: deps.ready,
    run: async (args) => {
      const T = deps.strings();
      const target = args === undefined ? deps.current() : argsTarget(args);
      if (!target) return deps.notify("warning", T.open.noFile, T.open.noFileBody);
      if (target.dir) return deps.notify("warning", T.open.folder);
      const nested = deps.nested(target.path);
      if (nested !== undefined) return deps.notify("warning", T.open.nested(nested));
      let top: string;
      try {
        top = await deps.workTree(target.worktree);
      } catch (e) {
        return deps.notify("error", T.open.failed(e instanceof Error ? e.message : String(e)));
      }
      const path = repoRelative(top, target.path);
      if (path === undefined) return deps.notify("warning", T.open.outside);
      deps.open({ kind: "history", path }, T.title(path.split("/").pop() ?? path), target.worktree);
    },
  };
}

/** 프로젝트 창에 한 번 둔다: `git.showFileHistory` 를 등록한다. */
export function GitFileHistory({ workspace }: { workspace: EditorWorkspace }) {
  const T = useGitHistoryStrings();
  const notify = useNotify();
  const live = useRef({ T, notify });
  live.current = { T, notify };
  const actions = useMemo(
    () => [
      fileHistoryAction({
        strings: () => live.current.T,
        ready: () => gitDocumentSource.getState().source !== undefined,
        current: () => currentTarget(workspace),
        nested: (path) => nestedRepository(windowFileTree(), path),
        workTree: (worktree) => {
          const source = gitDocumentSource.getState().source;
          if (!source) return Promise.reject(new Error("Host is not connected"));
          return source.workTree(worktree);
        },
        open: (spec, title, worktree) => openGitDocument(workspace, spec, title, worktree),
        notify: (level, title, body) =>
          live.current.notify({ level, title, ...(body && { body }) }),
      }),
    ],
    // 이름(label)은 언어가 바뀌면 다시 등록해 바꾼다.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [workspace, T],
  );
  useRegisterActions(actions);
  return null;
}
