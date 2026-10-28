// 「Git 로그에서 그 커밋 선택」(액션 `git.log.reveal { rev }`, 13-m2-plan M2-18 — 행 사이 이음매). 다른 화면이 id 로 부른다: 파일
// 히스토리의 우클릭(M2-18), blame 주석 클릭(M2-17).
//
//   registry.run("git.log.reveal", { rev })   rev 는 리비전 문법(전체 · 짧은 해시, 브랜치 이름) — 로그가 커밋 해시로 푼다
//
// Git 로그 툴 윈도우(⌘9)를 띄우고 요청을 `logRevealRequests` 에 남긴다. 툴 윈도우(git-log-tool-window.tsx)가 마운트돼 있으면
// 곧바로, 아니면 마운트될 때 받아 그 커밋을 고르고 화면 안으로 굴리고 목록에 포커스를 준다(「프로젝트 뷰에서 선택」 과 같은 길 —
// src/file-tree/project-view.tsx).
//
// 지금 필터(브랜치 · 텍스트 · 작성자 · 날짜 · 경로)에 그 커밋이 없으면 — 받은 목록에 없고 `git.log` 가 끝났으면 — 모든 ref 에서
// 닿는지 `git.logPage` 한 행으로 묻는다. 닿으면 필터를 모두 풀고(브랜치 패널은 「모든 브랜치」) 다시 받아 고른 뒤 「필터를 풀었다」
// 알림, 어느 ref 에서도 닿지 않으면(stash · reflog 에만 남은 커밋 등) 필터를 그대로 두고 경고 알림이다. 필터를 남긴 채 「없다」
// 고만 하지 않는 것은 부르는 쪽(blame 클릭)의 뜻이 「그 커밋을 보여 달라」 이기 때문이다.

import type { Action } from "@pitwall/keymap";
import type { DockviewApi } from "dockview-react";
import { useMemo } from "react";
import { createStore } from "zustand/vanilla";

import type { EditorWorkspace } from "../../editor/editor-workspace";
import { useRegisterActions } from "../../keymap/keymap-provider";
import { showPanel } from "../../layout/project-layout";
import { useGitLogStrings } from "./strings";

/** Git 로그 툴 윈도우의 id(layout/project-layout.ts 의 PANELS). */
export const GIT_LOG_TOOL_WINDOW = "git";

/** 「로그에서 선택」 요청. 툴 윈도우가 받아 비운다(`rev` 를 지운다). */
export const logRevealRequests = createStore<{ rev: string | undefined; seq: number }>()(() => ({
  rev: undefined,
  seq: 0,
}));

/** Git 로그 툴 윈도우를 띄우고 `rev` 를 고르게 한다. */
export function revealInLog(api: DockviewApi | undefined, rev: string) {
  if (api) showPanel(api, GIT_LOG_TOOL_WINDOW);
  logRevealRequests.setState((s) => ({ rev, seq: s.seq + 1 }));
}

/** `git.log.reveal`. 인자가 있어야만 돈다(키 · 메뉴 · 액션 찾기에는 없다). 인자의 꼴이 틀리면 아무것도 하지 않는다. */
export function logRevealAction(label: string, api: () => DockviewApi | undefined): Action {
  return {
    id: "git.log.reveal",
    label,
    args: true,
    run: (args) => {
      const rev =
        typeof args === "object" && args !== null ? (args as { rev?: unknown }).rev : undefined;
      if (typeof rev !== "string" || rev.trim() === "") {
        console.warn("[git] git.log.reveal needs { rev }", args);
        return;
      }
      revealInLog(api(), rev.trim());
    },
  };
}

/** 프로젝트 창에 한 번 둔다: `git.log.reveal` 을 등록한다. */
export function GitLogReveal({ workspace }: { workspace: EditorWorkspace }) {
  const label = useGitLogStrings().reveal.action;
  const actions = useMemo(
    () => [logRevealAction(label, () => workspace.api())],
    [label, workspace],
  );
  useRegisterActions(actions);
  return null;
}
