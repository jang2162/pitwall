// 프로젝트 창의 커밋 기능(docs/design/13-m2-plan.md M2-16)을 한데 붙인다: 쓰기(commit-ops.ts) · 메시지 기록(프로젝트 상태)을 창 상태에
// 걸고(commit-state.ts `commitFeature`), 액션을 등록하고, 활성 워크트리가 바뀌면 템플릿 · stash 를 다시 읽는다. 프로젝트 창에 하나 둔다.
//
//   git.commit            ⌘K   커밋 툴 윈도우를 열고 메시지 칸으로
//   git.commit.run        ⌘↩   커밋(커밋 툴 윈도우 안에서만 — `activeToolWindow == 'commit'`)
//   git.commitAndPush     ⌘⌥K  커밋 후 푸시 다이얼로그(M2-20 의 `git.push.open`). 메시지 칸이 비었으면 ⌘K 와 같다
//   git.rollback          ⌘⌥Z  변경 되돌리기(확인 뒤) — 커밋 툴 윈도우에서 고른 파일, 그 밖이면 활성 에디터 탭의 파일
//
// 확인 · 이름 묻기는 M1 의 대화상자(file-tree/file-dialogs.tsx)다 — 위험 작업 다이얼로그(M2-20)가 들어오면 바꾼다.

import type { Action, ActionRegistry } from "@pitwall/keymap";
import type { DockviewApi } from "dockview-react";
import { useEffect, useMemo, useRef } from "react";

import type { EditorWorkspace } from "../../editor/editor-workspace";
import { useFileDialogs } from "../../file-tree/file-dialogs";
import { useKeymap, useRegisterActions } from "../../keymap/keymap-provider";
import { showPanel } from "../../layout/project-layout";
import type { ProjectState } from "../../layout/project-state";
import { useNotify } from "../../notifications/notifications";
import { activeWorktree, useActiveWorktree } from "../../open-with/active-worktree";
import { useHost } from "../../rpc/host";
import { windowGitStatus } from "../git-status";
import { type CommitGroups, type CommitItem, groupEntries } from "./commit-model";
import { createCommitOps } from "./commit-ops";
import {
  type CommitFeature,
  commitFeature,
  commitPanel,
  fillTemplate,
  focusCommitMessage,
  reloadStashes,
  runCommit,
} from "./commit-state";
import { itemsOf } from "./commit-tool-window";
import { type GitCommitStrings, useGitCommitStrings } from "./strings";

/** 포커스가 커밋 툴 윈도우 안에 있다. */
function focusInCommitWindow(): boolean {
  return document.activeElement?.closest('[data-tool-window="commit"]') != null;
}

/** 되돌릴 항목: 커밋 툴 윈도우에서 고른 것, 그 밖이면 활성 에디터 탭 파일의 스테이징됨 · 변경 항목. */
export function rollbackTargets(
  groups: CommitGroups,
  selected: readonly string[],
  inWindow: boolean,
  activeFile: string | undefined,
): CommitItem[] {
  if (inWindow && selected.length > 0) return itemsOf(groups, selected);
  if (activeFile === undefined) return [];
  const rel = windowGitStatus().relative(activeFile);
  if (rel === undefined || rel === "") return [];
  return [...groups.staged, ...groups.changes].filter((i) => i.path === rel);
}

export function commitActions({
  strings,
  feature,
  open,
  activeFile,
}: {
  strings: GitCommitStrings["actions"];
  feature: () => CommitFeature | undefined;
  /** 커밋 툴 윈도우를 띄운다. */
  open(): void;
  activeFile(): string | undefined;
}): Action[] {
  const openAndFocus = () => {
    open();
    focusCommitMessage();
  };
  return [
    {
      id: "git.commit",
      label: strings.open,
      keywords: ["Commit", "Git Commit", "VCS"],
      run: openAndFocus,
    },
    {
      id: "git.commit.run",
      label: strings.run,
      keywords: ["Commit"],
      enabled: () => feature() !== undefined,
      run: async () => {
        const f = feature();
        if (f) await runCommit(f, false);
      },
    },
    {
      id: "git.commitAndPush",
      label: strings.runAndPush,
      keywords: ["Commit and Push", "Push"],
      run: async () => {
        const f = feature();
        if (!f || commitPanel.getState().message.trim() === "") return openAndFocus();
        await runCommit(f, true);
      },
    },
    {
      id: "git.rollback",
      label: strings.rollback,
      keywords: ["Rollback", "Revert Changes", "Discard"],
      enabled: () => feature() !== undefined,
      run: async () => {
        const f = feature();
        if (!f) return;
        const groups = groupEntries(windowGitStatus().entries());
        const items = rollbackTargets(
          groups,
          commitPanel.getState().selected,
          focusInCommitWindow(),
          activeFile(),
        );
        await f.ops.rollback(items);
      },
    },
  ];
}

export function GitCommitFeature({
  projectId,
  state,
  workspace,
  api,
}: {
  projectId: string;
  state: ProjectState;
  workspace: EditorWorkspace;
  api: DockviewApi | undefined;
}) {
  const T = useGitCommitStrings();
  const { client } = useHost();
  const { registry } = useKeymap();
  const notify = useNotify();
  const dialogs = useFileDialogs();
  const worktree = useActiveWorktree();

  // 동작은 누른 때의 값을 읽는다(Host 연결 · 문구 · 대화상자 · dock).
  const live = useRef({ client, T, dialogs, notify, api });
  live.current = { client, T, dialogs, notify, api };
  const feature = useMemo(
    () => createFeature(projectId, state, registry, live),
    [projectId, state, registry],
  );
  useEffect(() => {
    commitFeature.setState({ current: feature });
    return () => {
      if (commitFeature.getState().current === feature)
        commitFeature.setState({ current: undefined });
    };
  }, [feature]);

  // 활성 워크트리(저장소)가 바뀌면 템플릿 · stash 를 다시 읽는다.
  useEffect(() => {
    if (!client) return;
    void fillTemplate(feature);
    void reloadStashes(feature);
  }, [feature, client, worktree]);

  const actions = useMemo(
    () =>
      commitActions({
        strings: T.actions,
        feature: () => commitFeature.getState().current,
        open: () => {
          const dock = live.current.api;
          if (dock) showPanel(dock, "commit");
        },
        activeFile: () => workspace.activeContext()?.file,
      }),
    [T, workspace],
  );
  useRegisterActions(actions);
  return <>{dialogs.element}</>;
}

function createFeature(
  projectId: string,
  state: ProjectState,
  registry: ActionRegistry,
  live: {
    current: {
      client: ReturnType<typeof useHost>["client"];
      T: GitCommitStrings;
      dialogs: ReturnType<typeof useFileDialogs>;
      notify: ReturnType<typeof useNotify>;
    };
  },
): CommitFeature {
  const ops = createCommitOps({
    client: () => live.current.client,
    projectId,
    worktree: activeWorktree,
    confirm: (q) => live.current.dialogs.confirm(q),
    askName: (q) => live.current.dialogs.askName(q),
    notify: (n) => live.current.notify(n),
    strings: () => live.current.T,
  });
  return {
    ops,
    registry,
    strings: () => live.current.T,
    notify: (n) => live.current.notify(n),
    history: () => state.commitMessages(),
    remember: (message) => state.addCommitMessage(message),
    hasStaged: () =>
      windowGitStatus()
        .entries()
        .some((e) => e.staged !== null && !e.conflict),
  };
}
