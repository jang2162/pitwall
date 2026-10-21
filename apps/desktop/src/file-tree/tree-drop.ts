// 파일 트리로 끌어 놓기(M1-21, 10-ui §5.4): 놓을 폴더와 Finder(다른 앱)에서의 드롭. 트리 안 드래그는 tree-drag.ts.
//
// Finder 드롭은 웹뷰의 HTML 드롭이 경로를 주지 않아 데스크톱이 받는다(apps/desktop/src-tauri/src/drop_import.rs): 창이 드롭을 받으면
// 번호를 붙여 `pitwall://file-drop` 으로 알리고, 여기서 위치로 대상 폴더를 골라 `import_drop` 에 번호를 넘긴다 — 경로 글자를 넘기지
// 않는다(웹뷰가 아무 파일이나 고르지 못하게). 같은 드롭을 터미널도 받지만(경로 붙여넣기) 둘은 위치로 갈린다.

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

import type { DropImported } from "./file-ops";
import { parentOf, type TreeEntry } from "./tree-model";

/** 데스크톱이 드롭을 기록했다(drop_import.rs `DROP_EVENT` · `DropRecorded`). */
export const FILE_DROP_EVENT = "pitwall://file-drop";

export interface FileDrop {
  id: number;
  paths: string[];
  /** 창 안의 물리 픽셀 위치. */
  x: number;
  y: number;
}

export interface DropNative {
  listen(listener: (drop: FileDrop) => void): Promise<() => void>;
  importDrop(args: {
    dropId: number;
    projectId: string;
    worktree: string | undefined;
    dir: string;
    title: string;
  }): Promise<DropImported>;
}

export const tauriDropNative: DropNative = {
  // Tauri 밖(시험 · 웹)에서는 창을 얻다 던진다 — async 라 거절된 약속으로 바뀐다.
  listen: async (listener) =>
    getCurrentWebviewWindow().listen<FileDrop>(FILE_DROP_EVENT, ({ payload }) => listener(payload)),
  importDrop: ({ dropId, projectId, worktree, dir, title }) =>
    invoke<DropImported>("import_drop", {
      dropId,
      indices: null,
      projectId,
      worktree: worktree ?? null,
      dir,
      title,
    }),
};

/** 항목 위에 놓으면 들어갈 폴더: 폴더면 그 폴더, 파일(과 루트 밖 링크)이면 그 부모. */
export function folderOf(entry: TreeEntry, root: string): string {
  if (entry.dir && !entry.outside) return entry.path;
  return parentOf(entry.path) ?? root;
}

/**
 * 창 안 위치(CSS 픽셀)에 놓으면 들어갈 폴더. 트리(`tree`) 밖이면 `undefined`, 행이 없는 빈 자리면 루트다. `entryAt` 은 행의
 * `data-path` 로 항목을 찾는다.
 */
export function dropFolderAt(
  tree: HTMLElement,
  point: { x: number; y: number },
  root: string,
  entryAt: (path: string) => TreeEntry | undefined,
): string | undefined {
  const rect = tree.getBoundingClientRect();
  if (
    rect.width === 0 ||
    point.x < rect.left ||
    point.x > rect.right ||
    point.y < rect.top ||
    point.y > rect.bottom
  )
    return undefined;
  const row = document.elementFromPoint(point.x, point.y)?.closest("[role=treeitem]");
  const path = row && tree.contains(row) ? row.getAttribute("data-path") : null;
  const entry = path === null ? undefined : entryAt(path);
  return entry ? folderOf(entry, root) : root;
}
