// 프로젝트 창의 활성 워크트리(04 F2-5). 창이 받은 열기 요청(project-open.ts)의 `worktree`(`ActiveWorktree.top`)를 기억해
// 외부 도구가 `{project_root}` 를 그 워크트리 기준으로 열고(openWith.run 의 `worktree`) 파일 읽기(fs.*)가 그 워크트리 안의
// 루트를 접근 범위로 쓰게 한다. 창마다 웹뷰가 따로라 창 하나의 store 로 둔다 — 바뀌면 구독하는 화면(파일 트리 등)이 다시 그린다.
//
// 워크트리 없이 파일을 연 요청은 메인 체크아웃으로 돌아온 것으로 본다(브랜치 팝업의 메인 체크아웃 전환은 `mainCheckout`).
// 폴더만 다시 연 요청은 창을 앞으로 가져오기만 해서(빈 요청) 여기에 오지 않는다 — 그 경우 워크트리는 바뀌지 않는다.
//
// **다른 머신의 체크아웃**(18-m7-plan M7-11): store 의 `top` 은 체크아웃 열쇠(checkout/checkout.ts — 다른 머신이면 머신이 붙은
// 꼴)다. 기능(파일 트리 · git · DB · 감시 …)은 [`activeWorktree`] · [`useActiveWorktree`] 로 **그 머신의 워크트리 경로**를 받는다 — 그
// 기능은 활성 체크아웃의 연결(checkout/checkout-hosts.tsx)로 부르므로 머신을 따로 알 필요가 없다. 열쇠 자체가 필요한 곳(탭 묶음 ·
// 탭)은 [`activeCheckoutKey`] 를 쓴다.
//
// 프로젝트 창에서 활성 워크트리를 바꾸는 것은 워크트리 세션(layout/worktree-session.ts)이다 — 바꾸기 전에 지금 워크트리의 탭 묶음을
// 저장하고 탭을 닫는다(미저장 확인을 취소하면 바꾸지 않는다).

import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import {
  type Checkout,
  checkoutKey,
  checkoutOfKey,
  isRemotePath,
  LOCAL_MACHINE,
  qualify,
} from "../checkout/checkout";
import type { ProjectOpen } from "../project-open";

interface ActiveWorktreeState {
  /** 활성 체크아웃의 열쇠(활성 워크트리의 꼭대기 — 다른 머신이면 머신이 붙은 꼴). 이 머신의 메인 체크아웃이면 `undefined`. */
  top: string | undefined;
}

export const activeWorktreeStore = createStore<ActiveWorktreeState>()(() => ({ top: undefined }));

/** 요청이 가리키는 활성 워크트리(머리 주석의 규칙). 바꾸지 않는 요청(폴더만 다시 열기)이면 `{ change: false }`. */
export function requestedWorktree(
  open: ProjectOpen,
): { change: true; top: string | undefined } | { change: false } {
  // 체크아웃을 실은 요청(원격 셸의 `pitwall <파일>`)은 활성 체크아웃을 바꾸지 않고 그 체크아웃의 탭으로 연다(18-m7-plan M7-15 ② ·
  // M7-11b, layout/worktree-session.ts).
  if (open.checkout) return { change: false };
  if (open.worktree) return { change: true, top: open.worktree };
  if (open.file || open.mainCheckout) return { change: true, top: undefined };
  return { change: false };
}

export function rememberActiveWorktree(open: ProjectOpen): void {
  const next = requestedWorktree(open);
  if (next.change) activeWorktreeStore.setState({ top: next.top });
}

/** 지금 값 — 활성 체크아웃 머신의 워크트리 꼭대기. 키를 누른 때의 값을 읽는 쪽(액션 · RPC 인자)이 쓴다. */
export function activeWorktree(): string | undefined {
  return checkoutOfKey(activeWorktreeStore.getState().top).worktree;
}

/** 화면이 구독하는 값. 바뀌면 다시 그린다. */
export function useActiveWorktree(): string | undefined {
  return useStore(activeWorktreeStore, (s) => checkoutOfKey(s.top).worktree);
}

/** 활성 체크아웃의 열쇠(탭 묶음 · 탭의 `worktree` 와 같은 꼴). */
export function activeCheckoutKey(): string | undefined {
  return activeWorktreeStore.getState().top;
}

/** 활성 체크아웃. */
export function activeCheckout(): Checkout {
  return checkoutOfKey(activeWorktreeStore.getState().top);
}

/**
 * 열기 요청의 경로 · 워크트리를 창 안의 꼴로(18-m7-plan M7-11 리뷰 B2). **접두 없는 경로는 `machine` 의 것이다** — 기본은 활성
 * 체크아웃의 머신이다: 활성 체크아웃 아래 기능(파일 트리 · 찾기 · git · DB · 터미널 링크 · LSP 이동 …)은 그 머신 연결로 돌아 그
 * 머신의 경로를 접두 없이 넘긴다. 이미 접두가 있으면(다른 머신의 탭 · 그 저장본) 그대로다. 이 머신이면 그대로다(옛 꼴).
 * 에디터의 열기 입구(`editor-workspace.tsx` `open`)와 파일 탭을 만드는 `openTab` 이 부른다 — 열기는 모두 이 둘을 지난다.
 */
export function qualifyOpen(
  path: string,
  worktree: string | undefined,
  machine: string = activeCheckout().machine,
): { path: string; worktree: string | undefined } {
  if (isRemotePath(path) || machine === LOCAL_MACHINE) return { path, worktree };
  return {
    path: qualify(machine, path),
    worktree: isRemotePath(worktree) ? worktree : checkoutKey({ machine, worktree }),
  };
}

/** 활성 체크아웃의 머신(화면이 구독). */
export function useActiveMachine(): string {
  return useStore(activeWorktreeStore, (s) => checkoutOfKey(s.top).machine);
}
