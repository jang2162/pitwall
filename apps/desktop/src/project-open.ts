// 프로젝트 창이 받는 열기 요청(src-tauri/src/windows.rs 의 ProjectOpen). 새로 만든 창은 쿼리로, 이미 열린 창은
// `project://open` 이벤트로 받는다. 받을 에디터가 아직 없어 지금은 로그만 남긴다(M0-10).

import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

/** src-tauri/src/windows.rs 의 `PROJECT_OPEN_EVENT`. */
export const PROJECT_OPEN_EVENT = "project://open";

export interface ProjectOpen {
  /** 활성 워크트리의 꼭대기(`ActiveWorktree.top`). */
  worktree?: string;
  /** 열 파일의 절대 경로. */
  file?: string;
  line?: number;
  col?: number;
}

/** 프로젝트 창의 쿼리(`?window=project&project=<id>&worktree=…&file=…&line=…&col=…`). */
export function parseProjectQuery(search: string): { projectId?: string; open: ProjectOpen } {
  const params = new URLSearchParams(search);
  const text = (key: string) => params.get(key) ?? undefined;
  const number = (key: string) => {
    const value = Number(params.get(key));
    return Number.isInteger(value) && value >= 1 ? value : undefined;
  };
  const open: ProjectOpen = {};
  const worktree = text("worktree");
  const file = text("file");
  const line = number("line");
  const col = number("col");
  if (worktree !== undefined) open.worktree = worktree;
  if (file !== undefined) open.file = file;
  if (line !== undefined) open.line = line;
  if (col !== undefined) open.col = col;
  return { projectId: text("project"), open };
}

/** 로그 한 줄용 표기. 비었으면 `undefined`. */
export function describeOpen(open: ProjectOpen): string | undefined {
  const parts: string[] = [];
  if (open.worktree) parts.push(`worktree ${open.worktree}`);
  if (open.file) {
    const at = [open.line, open.col].filter((n) => n !== undefined).join(":");
    parts.push(`file ${open.file}${at ? `:${at}` : ""}`);
  }
  return parts.length > 0 ? parts.join(", ") : undefined;
}

/** 이 창을 대상으로 보낸 열기 요청을 듣는다. 해제 함수를 돌려준다. */
export function listenProjectOpen(listener: (open: ProjectOpen) => void): () => void {
  let disposed = false;
  let unlisten: (() => void) | undefined;
  getCurrentWebviewWindow()
    .listen<ProjectOpen>(PROJECT_OPEN_EVENT, (e) => listener(e.payload))
    .then((off) => (disposed ? off() : (unlisten = off)))
    .catch((e: unknown) => console.warn("[project] open listen failed", e));
  return () => {
    disposed = true;
    unlisten?.();
  };
}
