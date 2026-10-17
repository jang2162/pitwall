// 웰컴 창이 네이티브(Tauri)와 주고받는 것을 한 곳에 모은다. 시험 · 브라우저에서는 이 인터페이스로 갈아 끼운다.
//
// 프로젝트 창 열기와 「파일로 실행 → 프로젝트 찾기」 보류 요청은 src-tauri 가 만든다(M0-10 과의 계약):
// - 명령 `open_project_window` { projectId, worktree?, file?, line?, col? } — 그 프로젝트 창을 열거나 앞으로 가져오고
//   `project.markOpened` 를 갱신한다. `worktree` 는 활성 워크트리의 꼭대기(`ActiveWorktree.top`), `file` 은 절대 경로.
// - 이벤트 `find-project-requested` + 명령 `take_pending_find_project` → { path, line?, col? } | null. 꺼내면 비워진다.
//   웰컴 창은 뜰 때와 이벤트를 받을 때 꺼내 본다 — 창이 뜨기 전에 온 요청도 놓치지 않게.

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

export const FIND_PROJECT_EVENT = "find-project-requested";

export interface OpenProjectWindowArgs {
  projectId: string;
  /** 활성 워크트리의 꼭대기(`ActiveWorktree.top`). */
  worktree?: string;
  /** 창이 열 파일(절대 경로). */
  file?: string;
  line?: number;
  col?: number;
}

/** `pitwall <path>[:line[:col]]` 이 프로젝트를 못 찾았을 때 남긴 요청(F2). */
export interface FindProjectRequest {
  path: string;
  line?: number | null;
  col?: number | null;
}

export type DragDropEvent =
  | { kind: "enter" }
  | { kind: "leave" }
  | { kind: "drop"; paths: string[] };

export interface WelcomeNative {
  openProjectWindow(args: OpenProjectWindowArgs): Promise<void>;
  takePendingFindProject(): Promise<FindProjectRequest | null>;
  /** 보류 요청이 생겼다는 알림. 해제 함수를 돌려준다. */
  onFindProjectRequested(listener: () => void): () => void;
  /** OS 폴더 대화상자. 취소하면 `null`. */
  pickFolder(defaultPath?: string): Promise<string | null>;
  /** 파일 관리자(Finder 등)에서 그 경로를 보여 준다. */
  reveal(path: string): Promise<void>;
  /** 창에 끌어다 놓은 파일 · 폴더. 해제 함수를 돌려준다. */
  onDragDrop(listener: (event: DragDropEvent) => void): () => void;
}

/** Tauri 의 `listen` 은 Promise 로 해제 함수를 준다. 받기 전에 해제되면 받는 즉시 푼다. */
function lazyUnlisten(start: () => Promise<() => void>, what: string): () => void {
  let disposed = false;
  let unlisten: (() => void) | undefined;
  start()
    .then((off) => (disposed ? off() : (unlisten = off)))
    .catch((e: unknown) => console.warn(`[welcome] ${what} listen failed`, e));
  return () => {
    disposed = true;
    unlisten?.();
  };
}

export const tauriWelcomeNative: WelcomeNative = {
  openProjectWindow: (args) => invoke("open_project_window", { ...args }),
  takePendingFindProject: () => invoke<FindProjectRequest | null>("take_pending_find_project"),
  onFindProjectRequested: (listener) =>
    lazyUnlisten(
      () => getCurrentWebviewWindow().listen(FIND_PROJECT_EVENT, () => listener()),
      FIND_PROJECT_EVENT,
    ),
  pickFolder: async (defaultPath) => {
    const picked = await open({ directory: true, multiple: false, defaultPath });
    return typeof picked === "string" ? picked : null;
  },
  reveal: (path) => revealItemInDir(path),
  onDragDrop: (listener) =>
    lazyUnlisten(
      () =>
        getCurrentWebview().onDragDropEvent(({ payload }) => {
          if (payload.type === "enter") listener({ kind: "enter" });
          else if (payload.type === "leave") listener({ kind: "leave" });
          else if (payload.type === "drop") listener({ kind: "drop", paths: payload.paths });
        }),
      "drag-drop",
    ),
};
