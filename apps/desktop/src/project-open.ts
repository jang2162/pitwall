// 프로젝트 창이 받는 열기 요청(src-tauri/src/windows.rs 의 ProjectOpen · PendingOpens). 요청은 Rust 의 창별 큐에 쌓이고,
// 창은 마운트할 때와 `project://open` 알림을 받을 때 `take_pending_project_opens` 로 꺼낸다. 새 창의 첫 요청도 같은 큐로
// 온다(쿼리에는 프로젝트 id 만 있다). 꺼낸 요청은 `projectOpenInbox` 에 넣고, 창의 에디터가 마운트되면 그것을 받아 탭을
// 연다(src/editor/editor-workspace.ts) — 꺼내기는 React 보다 먼저(main.tsx) 시작하므로 그사이 온 요청은 inbox 가 들고 있다.

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

/** src-tauri/src/windows.rs 의 `PROJECT_OPEN_EVENT`. payload 는 없다. */
export const PROJECT_OPEN_EVENT = "project://open";

export interface ProjectOpen {
  /** 활성 워크트리의 꼭대기(`ActiveWorktree.top`). */
  worktree?: string;
  /** 열 파일의 절대 경로. 디렉터리는 Rust 가 걸러 오지 않는다. */
  file?: string;
  /** 1부터. */
  line?: number;
  /** 1부터(글자 단위). */
  col?: number;
}

/** 프로젝트 창의 쿼리(`?window=project&project=<id>`)에서 프로젝트 id. */
export function projectIdFromQuery(search: string): string | undefined {
  return new URLSearchParams(search).get("project") ?? undefined;
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

/** 큐와 알림. 시험은 가짜를 넣는다. */
export interface ProjectOpenSource {
  /** 이 창에 온 알림을 듣는다. 해제 함수로 끝난다. */
  listen(onNotify: () => void): Promise<() => void>;
  /** 이 창의 큐를 꺼낸다(꺼내면 비운다). */
  take(): Promise<ProjectOpen[]>;
}

export const tauriProjectOpenSource: ProjectOpenSource = {
  listen: (onNotify) => getCurrentWebviewWindow().listen(PROJECT_OPEN_EVENT, () => onNotify()),
  take: () => invoke<ProjectOpen[]>("take_pending_project_opens"),
};

/**
 * 요청을 온 순서대로 `onOpen` 에 넘긴다. **리스너를 건 뒤에 처음 꺼낸다** — 리스너가 없던 사이(창을 만드는 중)에 쌓인 요청은
 * 그 꺼내기가 받고, 그 뒤의 요청은 알림이 부르는 꺼내기가 받는다. 꺼내기는 하나씩 차례로 한다(순서를 지킨다).
 * 해제 함수를 돌려준다.
 */
export function watchProjectOpens(
  source: ProjectOpenSource,
  onOpen: (open: ProjectOpen) => void,
  onError: (e: unknown) => void = (e) => console.warn("[project] open requests", e),
): () => void {
  let disposed = false;
  let unlisten: (() => void) | undefined;
  let chain = Promise.resolve();
  const drain = () => {
    chain = chain
      .then(() => (disposed ? [] : source.take()))
      .then((opens) => opens.forEach(onOpen))
      .catch(onError);
  };
  source
    .listen(drain)
    .then((off) => {
      if (disposed) off();
      else unlisten = off;
      drain();
    })
    .catch((e: unknown) => {
      // 알림은 못 받아도 이미 쌓인 요청(새 창의 첫 요청 등)은 꺼낸다.
      onError(e);
      drain();
    });
  return () => {
    disposed = true;
    unlisten?.();
  };
}

/**
 * 받을 곳이 생길 때까지 요청을 모아 두는 우편함. 받는 쪽은 `connect` 로 붙고(그때 모인 것을 온 순서대로 받는다), 해제 함수로 뗀다.
 * 붙은 쪽이 없는 동안 온 요청은 다시 모인다. 받는 쪽은 하나다 — 새로 붙으면 앞의 것을 대신한다.
 */
export function createOpenInbox() {
  const waiting: ProjectOpen[] = [];
  let receiver: ((open: ProjectOpen) => void) | undefined;
  return {
    push(open: ProjectOpen) {
      if (receiver) receiver(open);
      else waiting.push(open);
    },
    connect(onOpen: (open: ProjectOpen) => void): () => void {
      receiver = onOpen;
      for (const open of waiting.splice(0)) onOpen(open);
      return () => {
        if (receiver === onOpen) receiver = undefined;
      };
    },
  };
}

export type OpenInbox = ReturnType<typeof createOpenInbox>;

/** 이 창의 우편함. main.tsx 가 넣고 에디터가 꺼낸다. */
export const projectOpenInbox: OpenInbox = createOpenInbox();
