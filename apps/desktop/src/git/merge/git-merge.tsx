// 프로젝트 창의 충돌 해결(13-m2-plan M2-19)을 한데 붙인다: 이 창의 Host 연결로 부르는 길(source.ts)을 걸고, 액션을 등록하고, 충돌
// 목록 다이얼로그와 확인 대화상자를 단다. 진행 중 작업 배너(operation-banner.tsx)는 자리가 따로라 창이 그 자리에 단다.
//
//   git.resolveConflicts                 충돌 목록 다이얼로그(액션 찾기에 보인다)
//   git.openMerge { path }               그 파일의 3-way 머지 문서 탭(인자가 있어야만 돈다). `path` 는 저장소 꼭대기 기준(`/`)
//   git.continueOperation · skipOperation · abortOperation
//                                        진행 중 작업을 계속 · 이 단계 건너뛰기 · 중단(`git.operation.continue` · `skip` · `abort`).
//                                        그 작업이 할 수 있을 때만 켜진다. 중단은 확인 뒤에만(결정 M2-D6 ①)
//
// 중단 확인은 지금 M1 의 확인 대화상자(file-tree/file-dialogs.tsx)다 — 위험 작업 다이얼로그 부품의 주인은 M2-20 이고 그것이 들어오면
// 바꾼다. 계속 · 건너뛰기가 다음 단계에서 다시 멈추면 경고 알림과 함께 충돌 목록을 연다. 끝나면 알림만 — [되돌리기] 는 M2-20.
// git 이 거절한 것(`2004` — 덜 푼 충돌로 continue 등)은 쓰기 Task 의 실패 알림이 이미 원문을 보였다 — 여기서 또 알리지 않는다.

import type { Action } from "@pitwall/keymap";
import {
  type OperationAction,
  type OperationInfo,
  RpcCallError,
  type RpcClient,
} from "@pitwall/rpc-client";
import { useEffect, useMemo, useRef, useState } from "react";

import type { EditorWorkspace } from "../../editor/editor-workspace";
import type { ConfirmQuestion } from "../../file-tree/file-ops";
import { useFileDialogs } from "../../file-tree/file-dialogs";
import { useRegisterActions } from "../../keymap/keymap-provider";
import { type NotificationInput, useNotify } from "../../notifications/notifications";
import { activeWorktree, useActiveWorktree } from "../../open-with/active-worktree";
import { useHost } from "../../rpc/host";
import { GIT_FAILED } from "../branch-ops";
import { windowBranch } from "../branch-state";
import { openGitDocument } from "../documents/git-documents";
import { ConflictsDialog } from "./conflicts-dialog";
import { dirtyMergeSessions } from "./merge-sessions";
import { fileName, type MergeSource, mergeSource } from "./source";
import { type GitMergeStrings, useGitMergeStrings } from "./strings";

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** 이 창의 Host 연결로 부르는 길. */
export function hostMergeSource(
  client: () => RpcClient | undefined,
  projectId: string,
): MergeSource {
  const host = () => {
    const c = client();
    if (!c) throw new Error("Host is not connected");
    return c;
  };
  const scope = (worktree: string | undefined) => ({ projectId, worktree: worktree ?? null });
  return {
    conflicts: (worktree) => host().call("git.conflicts", scope(worktree)),
    conflict: (path, worktree) => host().call("git.conflict.get", { ...scope(worktree), path }),
    resolve: (params, worktree) =>
      host().call("git.conflict.resolve", { ...scope(worktree), ...params }),
    operation: (action, worktree) => {
      switch (action) {
        case "continue":
          return host().call("git.operation.continue", scope(worktree));
        case "skip":
          return host().call("git.operation.skip", scope(worktree));
        case "abort":
          return host().call("git.operation.abort", scope(worktree));
      }
    },
  };
}

export interface MergeActionDeps {
  source(): MergeSource | undefined;
  /** 지금 진행 중 작업(창의 브랜치 상태). */
  operation(): OperationInfo | null;
  strings(): GitMergeStrings;
  confirm(question: ConfirmQuestion): Promise<boolean>;
  notify(input: NotificationInput): void;
  openConflicts(): void;
  /** 결과 칸을 고쳤지만 저장하지 않은 머지 탭 수(중단 확인의 경고). */
  dirtyMerges(): number;
  openMerge(path: string, worktree: string | undefined): void;
}

/** 충돌 해결 액션. 시험은 가짜를 넘긴다. */
export function mergeActions(deps: MergeActionDeps): Action[] {
  const T = deps.strings();
  const ready = () => deps.source() !== undefined;
  const can = (action: OperationAction) => () =>
    ready() && (deps.operation()?.actions.includes(action) ?? false);

  const runOperation = (action: OperationAction, label: string) => async () => {
    const S = deps.strings();
    const op = deps.operation();
    const source = deps.source();
    if (!op || !source) return;
    if (action === "abort") {
      const dirty = deps.dirtyMerges();
      const ok = await deps.confirm({
        title: S.abort.title(op.kind),
        body: S.abort.body,
        ...(dirty > 0 && { warning: S.abort.unsaved(dirty) }),
        confirm: S.abort.confirm,
        danger: true,
      });
      if (!ok) return;
    }
    try {
      const result = await source.operation(action, activeWorktree());
      if (result.stopped) {
        deps.notify({ level: "warning", title: S.results.stopped });
        deps.openConflicts();
      } else if (result.operation === null) {
        deps.notify({
          level: "info",
          title: action === "abort" ? S.results.aborted(op.kind) : S.results.done(op.kind),
        });
      } else if (action === "skip") {
        deps.notify({ level: "info", title: S.results.skipped });
      }
    } catch (e) {
      if (e instanceof RpcCallError && e.code === GIT_FAILED) return;
      deps.notify({ level: "error", title: S.results.failed(label), body: errorText(e) });
    }
  };

  return [
    {
      id: "git.resolveConflicts",
      label: T.actions.resolveConflicts,
      keywords: ["Resolve Conflicts", "Merge Conflicts", "Conflicts"],
      enabled: ready,
      run: () => deps.openConflicts(),
    },
    {
      id: "git.openMerge",
      label: T.actions.open,
      args: true,
      enabled: ready,
      run: (args) => {
        const path =
          typeof args === "object" && args !== null ? (args as { path?: unknown }).path : undefined;
        if (typeof path !== "string" || path === "" || path.startsWith("/")) {
          deps.notify({ level: "warning", title: deps.strings().results.failed(T.actions.open) });
          return;
        }
        deps.openMerge(path, activeWorktree());
      },
    },
    {
      id: "git.continueOperation",
      label: T.actions.continue,
      keywords: ["Continue Merge", "Continue Rebase", "Continue"],
      enabled: can("continue"),
      run: runOperation("continue", T.actions.continue),
    },
    {
      id: "git.skipOperation",
      label: T.actions.skip,
      keywords: ["Skip Commit", "Skip"],
      enabled: can("skip"),
      run: runOperation("skip", T.actions.skip),
    },
    {
      id: "git.abortOperation",
      label: T.actions.abort,
      keywords: ["Abort Merge", "Abort Rebase", "Abort"],
      enabled: can("abort"),
      run: runOperation("abort", T.actions.abort),
    },
  ];
}

/** 프로젝트 창에 한 번 둔다. */
export function GitMerge({
  projectId,
  workspace,
}: {
  projectId: string;
  workspace: EditorWorkspace;
}) {
  const T = useGitMergeStrings();
  const { client } = useHost();
  const notify = useNotify();
  const dialogs = useFileDialogs();
  const worktree = useActiveWorktree();
  const [conflictsOpen, setConflictsOpen] = useState(false);

  const live = useRef({ client, T, dialogs, notify });
  live.current = { client, T, dialogs, notify };

  const source = useMemo(() => hostMergeSource(() => live.current.client, projectId), [projectId]);
  useEffect(() => {
    mergeSource.setState({ source });
    return () => {
      if (mergeSource.getState().source === source) mergeSource.setState({ source: undefined });
    };
  }, [source]);

  const actions = useMemo(
    () =>
      mergeActions({
        source: () => mergeSource.getState().source,
        operation: () => windowBranch().get().operation,
        strings: () => live.current.T,
        confirm: (q) => live.current.dialogs.confirm(q),
        notify: (n) => live.current.notify(n),
        openConflicts: () => setConflictsOpen(true),
        dirtyMerges: () => dirtyMergeSessions().length,
        openMerge: (path, wt) =>
          openGitDocument(
            workspace,
            { kind: "merge", path },
            live.current.T.doc.title(fileName(path)),
            wt,
          ),
      }),
    // 문구가 바뀌면 액션 이름도 바뀐다.
    [workspace, T],
  );
  useRegisterActions(actions);

  return (
    <>
      <ConflictsDialog
        open={conflictsOpen}
        onOpenChange={setConflictsOpen}
        source={source}
        worktree={worktree}
        strings={T}
        onMerge={(path) =>
          openGitDocument(
            workspace,
            { kind: "merge", path },
            T.doc.title(fileName(path)),
            activeWorktree(),
          )
        }
        notify={(level, title, body) => notify({ level, title, ...(body && { body }) })}
      />
      {dialogs.element}
    </>
  );
}
