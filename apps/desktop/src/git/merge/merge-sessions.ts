// 머지 문서 탭(merge-document.tsx)의 세션: 탭(dockview 패널 id)마다 하나, 그 탭이 닫힐 때까지 산다. 읽은 충돌 · 저장에 실을
// `version` · 결과 칸의 CodeMirror 상태(글 · undo 기록 · 구간 상태) · base 토글을 들고 있다 — 탭의 내용 컴포넌트가 다시 그려지거나
// 다시 마운트돼도(탭 고정 · 언어 · 테마 · 그룹 사이로 옮김 · StrictMode) 결과 칸은 여기서 이어지고, 처음 결과로 돌아가지 않는다.
// 창마다 웹뷰가 따로라 모듈 상태로 둔다(src/git/git-status.ts 와 같다).
//
// 결과 칸이 처음 글(자동 병합 결과)과 다르면 미저장이다 — 문서가 작업 공간에 미저장 탭으로 알려(editor-workspace.tsx
// `markUnsavedTab`) 탭을 닫기 · 창 닫기 · 워크트리 전환 전에 에디터 탭과 같은 확인을 묻는다.

import type { EditorState } from "@codemirror/state";
import type { GitConflictDetail } from "@pitwall/rpc-client";

import type { MergeInput } from "./merge-model";

export type Loaded =
  | { kind: "loading" }
  | { kind: "failed"; message: string }
  | { kind: "gone" }
  | {
      kind: "ready";
      detail: GitConflictDetail;
      language: string | null;
      /** 자동 병합 결과가 없으면(한쪽 고르기만) 없다. */
      input: MergeInput | undefined;
    };

export interface MergeSession {
  readonly panelId: string;
  readonly path: string;
  readonly worktree: string | undefined;
  loaded: Loaded;
  /** 저장에 싣는 `expectedVersion`(get 이 준 것, CONFLICT 뒤 다시 읽은 것). */
  version: string | null;
  /** 결과 칸의 마지막 상태. 아직 칸을 만들지 않았으면 없다. */
  state: EditorState | undefined;
  base: boolean;
  /** 지금 떠 있는 문서의 저장(미저장 확인의 [저장]). 풀었으면 `true`. 떠 있는 문서가 없으면 없다. */
  save: (() => Promise<boolean>) | undefined;
}

const sessions = new Map<string, MergeSession>();

/** 그 탭의 세션. 없거나 다른 파일의 것이면 새로 만든다. */
export function mergeSession(
  panelId: string,
  path: string,
  worktree: string | undefined,
): MergeSession {
  const held = sessions.get(panelId);
  if (held && held.path === path && held.worktree === worktree) return held;
  const fresh: MergeSession = {
    panelId,
    path,
    worktree,
    loaded: { kind: "loading" },
    version: null,
    state: undefined,
    base: false,
    save: undefined,
  };
  sessions.set(panelId, fresh);
  return fresh;
}

/** 탭이 닫혔다. */
export function dropMergeSession(panelId: string) {
  sessions.delete(panelId);
}

/** 결과 칸이 처음 글과 다르다. */
export function isDirty(session: MergeSession): boolean {
  const { loaded, state } = session;
  if (loaded.kind !== "ready" || !loaded.input || !state) return false;
  const start = loaded.input.docs.result;
  return state.doc.length !== start.length || state.doc.toString() !== start;
}

/** 결과 칸을 고친 머지 탭들(진행 중 작업 [중단] 의 경고). */
export function dirtyMergeSessions(): MergeSession[] {
  return [...sessions.values()].filter(isDirty);
}

/** 시험용. */
export function resetMergeSessions() {
  sessions.clear();
}
