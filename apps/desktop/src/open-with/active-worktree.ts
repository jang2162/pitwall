// 프로젝트 창의 활성 워크트리(04 F2-5). 창이 받은 열기 요청(project-open.ts)의 `worktree`(`ActiveWorktree.top`)를 기억해
// 외부 도구가 `{project_root}` 를 그 워크트리 기준으로 열고(openWith.run 의 `worktree`) 파일 읽기(fs.*)가 그 워크트리 안의
// 루트를 접근 범위로 쓰게 한다. 창마다 웹뷰가 따로라 창 하나의 store 로 둔다 — 바뀌면 구독하는 화면(파일 트리 등)이 다시 그린다.
//
// 워크트리 없이 파일을 연 요청은 메인 체크아웃으로 돌아온 것으로 본다. 폴더만 다시 연 요청은 창을 앞으로 가져오기만 해서(빈 요청)
// 여기에 오지 않는다 — 그 경우 워크트리는 바뀌지 않는다.

import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import type { ProjectOpen } from "../project-open";

interface ActiveWorktreeState {
  /** 활성 워크트리의 꼭대기. 메인 체크아웃이면 `undefined`. */
  top: string | undefined;
}

export const activeWorktreeStore = createStore<ActiveWorktreeState>()(() => ({ top: undefined }));

export function rememberActiveWorktree(open: ProjectOpen): void {
  if (open.worktree) activeWorktreeStore.setState({ top: open.worktree });
  else if (open.file) activeWorktreeStore.setState({ top: undefined });
}

/** 지금 값. 키를 누른 때의 값을 읽는 쪽(액션 · RPC 인자)이 쓴다. */
export function activeWorktree(): string | undefined {
  return activeWorktreeStore.getState().top;
}

/** 화면이 구독하는 값. 바뀌면 다시 그린다. */
export function useActiveWorktree(): string | undefined {
  return useStore(activeWorktreeStore, (s) => s.top);
}
