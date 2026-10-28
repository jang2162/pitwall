// 프로젝트 창의 브랜치 기능(13-m2-plan.md M2-13)을 한데 붙인다: 창의 ref 읽기(branch-state.ts), 상태바의 브랜치 항목, 액션
// `git.branches`(⌃⇧B, 브랜치 팝업) · `git.vcsOperations`(⌃V, VCS 작업 팝업), Search Everywhere 의 브랜치 탭, 브랜치 이름 · 위험 작업
// 다이얼로그(M2-20, dialogs/git-dialogs.tsx — 추적 브랜치의 로컬 이름만 M1 의 이름 묻기)와 [되돌리기] 알림(dialogs/undo.ts).
// 프로젝트 창에 하나 둔다.
//
// 워크트리 전환은 창의 열기 요청으로 보낸다(project-open.ts 의 `projectOpenInbox`) — M1-25 의 워크트리 세션이 받아 탭 묶음을 저장하고
// 미저장을 묻고 새 워크트리의 묶음을 연다. 메인 체크아웃으로 돌아가는 요청은 `mainCheckout` 이다.

import type { Action } from "@pitwall/keymap";
import type { GitWorktree } from "@pitwall/rpc-client";
import { useMemo, useRef, useState } from "react";

import { useFileDialogs } from "../file-tree/file-dialogs";
import { useKeymap, useRegisterActions, useSearchSource } from "../keymap/keymap-provider";
import type { ProjectState } from "../layout/project-state";
import { useNotify } from "../notifications/notifications";
import { activeWorktree } from "../open-with/active-worktree";
import { projectOpenInbox } from "../project-open";
import { useHost } from "../rpc/host";
import { STATUS_SLOTS, useStatusBarItem } from "../status-bar/status-bar";
import { createBranchOps } from "./branch-ops";
import { BranchPopup, VcsPopup } from "./branch-popup";
import { branchSearchSource } from "./branch-search";
import { GitBranchWatch, windowBranch } from "./branch-state";
import { BranchStatus } from "./branch-status";
import { type BranchStrings, useBranchStrings } from "./branch-strings";
import { useGitDialogs } from "./dialogs/git-dialogs";
import { useGitDialogStrings } from "./dialogs/strings";
import { offerUndo } from "./dialogs/undo";

/** 고른 워크트리로 바꾸는 열기 요청. 바꿀 수 없는 워크트리면 아무것도 하지 않는다. */
export function switchWorktree(worktree: GitWorktree, push = projectOpenInbox.push) {
  const to = worktree.switchTo;
  if (!to || worktree.current) return;
  push(to.worktree === null ? { mainCheckout: true } : { worktree: to.worktree });
}

/** 브랜치 팝업(⌃⇧B) · VCS 작업 팝업(⌃V) 액션. 프로젝트 창에만 등록된다. */
export function branchActions({
  strings,
  openBranches,
  openVcs,
}: {
  strings: BranchStrings["actions"];
  openBranches(): void;
  openVcs(): void;
}): Action[] {
  return [
    {
      id: "git.branches",
      label: strings.branches,
      keywords: ["Branches", "Git Branches", "Checkout"],
      run: openBranches,
    },
    {
      id: "git.vcsOperations",
      label: strings.vcsOperations,
      keywords: ["VCS Operations", "Git"],
      run: openVcs,
    },
  ];
}

export function GitBranchFeature({ projectId, state }: { projectId: string; state: ProjectState }) {
  useStatusBarItem({ id: "branch", ...STATUS_SLOTS.branch, component: BranchStatus });
  const T = useBranchStrings();
  const { client } = useHost();
  const { registry } = useKeymap();
  const notify = useNotify();
  const dialogs = useFileDialogs();
  const gitDialogs = useGitDialogs();
  const D = useGitDialogStrings();
  const [popup, setPopup] = useState<"branches" | "vcs">();
  const [favorites, setFavorites] = useState<readonly string[]>([]);

  // 동작은 누른 때의 값을 읽는다(Host 연결 · 문구 · 대화상자).
  const live = useRef({ client, T, D, dialogs, gitDialogs, notify });
  live.current = { client, T, D, dialogs, gitDialogs, notify };
  const ops = useMemo(
    () =>
      createBranchOps({
        client: () => live.current.client,
        projectId,
        worktree: activeWorktree,
        refs: () => windowBranch().get().refs,
        askName: (q) => live.current.dialogs.askName(q),
        askBranch: (q) => live.current.gitDialogs.askBranch(q),
        confirm: (q) => live.current.gitDialogs.confirm(q),
        notify: (n) => live.current.notify(n),
        offerUndo: (move, kind, title) => {
          const { client, D, gitDialogs, notify } = live.current;
          if (!client) return notify({ level: "info", title });
          const worktree = activeWorktree();
          offerUndo(
            {
              client,
              projectId,
              ...(worktree !== undefined && { worktree }),
              confirm: gitDialogs.confirm,
              notify,
              strings: D,
            },
            move,
            kind,
            title,
          );
        },
        strings: () => live.current.T,
        dialogStrings: () => live.current.D,
        registry,
      }),
    [projectId, registry],
  );

  const actions = useMemo(
    () =>
      branchActions({
        strings: T.actions,
        openBranches: () => {
          setFavorites(state.favoriteBranches());
          setPopup("branches");
        },
        openVcs: () => setPopup("vcs"),
      }),
    [T, state],
  );
  useRegisterActions(actions);

  const source = useMemo(() => branchSearchSource(() => ops), [ops]);
  useSearchSource(source);

  const toggleFavorite = (ref: string) => {
    const now = state.favoriteBranches();
    const next = now.includes(ref) ? now.filter((r) => r !== ref) : [...now, ref];
    state.setFavoriteBranches(next);
    setFavorites(next);
  };

  return (
    <>
      <GitBranchWatch projectId={projectId} />
      <BranchPopup
        open={popup === "branches"}
        onOpenChange={(open) => setPopup(open ? "branches" : undefined)}
        projectId={projectId}
        ops={ops}
        favorites={{ list: favorites, toggle: toggleFavorite }}
        onSwitchWorktree={(w) => switchWorktree(w)}
      />
      <VcsPopup
        open={popup === "vcs"}
        onOpenChange={(open) => setPopup(open ? "vcs" : undefined)}
      />
      {dialogs.element}
      {gitDialogs.element}
    </>
  );
}
