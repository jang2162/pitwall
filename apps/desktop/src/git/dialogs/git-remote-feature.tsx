// 프로젝트 창의 원격 · 위험 작업 다이얼로그(13-m2-plan.md M2-20)를 한데 붙인다. 액션:
//
// - `git.push.open`(⌘⇧K): 푸시 다이얼로그. 인자 `{ branch?: string }` 로 보낼 브랜치를 고를 수 있다(없으면 HEAD 의 브랜치) — 커밋
//   툴 윈도우의 「커밋 후 푸시」(M2-16)가 이 id 로 부른다.
// - `git.pull.open`(⌘T): 풀 옵션 · 업데이트 다이얼로그.
// - `git.fetch`: 원격 전부를 가져온다(인증은 askpass 로 창에 묻는다).
// - `git.undo.offer`(인자가 있어야만 돈다): ref 를 옮긴 쓰기의 결과 `{ move: GitRefMove, mode?: "soft" | "mixed" | "keep" }` 를
//   알림으로 알리고 [되돌리기] 를 단다(undo.ts). 다른 행(amend 의 M2-16 등)이 이 부품을 import 하지 않고 부른다. mode 를 주지 않으면
//   keep 이다.
//
// 부르는 연결은 프로젝트 창의 연결이다 — 웰컴 · 설정 창은 git 쓰기를 부르지 않는다.

import type { Action } from "@pitwall/keymap";
import { type GitRefMove, RpcCallError } from "@pitwall/rpc-client";
import { useMemo, useRef, useState } from "react";

import { useKeymap, useRegisterActions } from "../../keymap/keymap-provider";
import { useNotify } from "../../notifications/notifications";
import { activeWorktree } from "../../open-with/active-worktree";
import { useHost } from "../../rpc/host";
import { windowBranch } from "../branch-state";
import { useGitDialogs } from "./git-dialogs";
import { PullDialog } from "./pull-dialog";
import { type PushInput, PushDialog } from "./push-dialog";
import { type GitDialogStrings, useGitDialogStrings } from "./strings";
import { GIT_FAILED, type UndoDeps, type UndoMode, errorText, offerUndo } from "./undo";

const MODES: readonly string[] = ["soft", "mixed", "keep"] satisfies UndoMode[];

function isMove(value: unknown): value is GitRefMove {
  if (typeof value !== "object" || value === null) return false;
  const m = value as Record<string, unknown>;
  const hash = (v: unknown) => v === null || typeof v === "string";
  return typeof m.refName === "string" && hash(m.previous) && hash(m.current);
}

/** `git.undo.offer` 의 인자. 맞지 않으면 `undefined`. */
export function undoOfferArgs(args: unknown): { move: GitRefMove; mode: UndoMode } | undefined {
  if (typeof args !== "object" || args === null) return undefined;
  const { move, mode } = args as { move?: unknown; mode?: unknown };
  if (!isMove(move)) return undefined;
  if (mode !== undefined && !MODES.includes(mode as string)) return undefined;
  return { move, mode: (mode as UndoMode | undefined) ?? "keep" };
}

export function remoteActions({
  strings,
  openPush,
  openPull,
  fetch,
  offer,
}: {
  strings: GitDialogStrings["actions"];
  openPush(input: PushInput): void;
  openPull(): void;
  fetch(): void;
  offer(args: unknown): void;
}): Action[] {
  return [
    {
      id: "git.push.open",
      label: strings.push,
      keywords: ["Push", "Git Push"],
      run: (args) => {
        const branch = (args as { branch?: unknown } | undefined)?.branch;
        openPush(typeof branch === "string" ? { branch } : {});
      },
    },
    {
      id: "git.pull.open",
      label: strings.pull,
      keywords: ["Update Project", "Pull", "Git Pull"],
      run: openPull,
    },
    { id: "git.fetch", label: strings.fetch, keywords: ["Fetch", "Git Fetch"], run: fetch },
    { id: "git.undo.offer", label: "git.undo.offer", args: true, run: offer },
  ];
}

export function GitRemoteFeature({ projectId }: { projectId: string }) {
  const T = useGitDialogStrings();
  const { client } = useHost();
  const { registry } = useKeymap();
  const notify = useNotify();
  const dialogs = useGitDialogs();
  const [open, setOpen] = useState<{ kind: "push"; input: PushInput } | { kind: "pull" }>();

  // 동작은 누른 때의 값을 읽는다.
  const live = useRef({ client, T, dialogs, notify });
  live.current = { client, T, dialogs, notify };
  const undoDeps = (): UndoDeps | undefined => {
    const { client, T, dialogs, notify } = live.current;
    if (!client) return undefined;
    const worktree = activeWorktree();
    return {
      client,
      projectId,
      ...(worktree !== undefined && { worktree }),
      confirm: dialogs.confirm,
      notify,
      strings: T,
    };
  };

  const actions = useMemo(
    () =>
      remoteActions({
        strings: T.actions,
        openPush: (input) => setOpen({ kind: "push", input }),
        openPull: () => setOpen({ kind: "pull" }),
        fetch: () => {
          const { client, T, notify } = live.current;
          if (!client) return;
          client.call("git.fetch", { projectId, worktree: activeWorktree() ?? null }).then(
            () => notify({ level: "info", title: T.fetch.done }),
            (e: unknown) => {
              // git 의 거절은 쓰기 Task 의 실패 알림이 원문을 이미 보였다.
              if (e instanceof RpcCallError && e.code === GIT_FAILED) return;
              notify({
                level: "error",
                title: T.common.failed(T.actions.fetch),
                body: errorText(e),
              });
            },
          );
        },
        offer: (args) => {
          const parsed = undoOfferArgs(args);
          const deps = undoDeps();
          if (!parsed || !deps) {
            console.warn("[git] git.undo.offer needs { move, mode? }", args);
            return;
          }
          offerUndo(deps, parsed.move, { kind: "head", mode: parsed.mode });
        },
      }),
    // `undoDeps` 는 `live` 를 읽는다 — 문구가 바뀔 때만 다시 등록한다.
    [T, projectId],
  );
  useRegisterActions(actions);

  const deps = undoDeps();
  return (
    <>
      {open?.kind === "push" && deps && (
        <PushDialog
          initial={open.input}
          onClose={() => setOpen(undefined)}
          deps={{
            ...deps,
            refs: () => windowBranch().get().refs,
            ...(registry.isEnabled("git.pull.open") && {
              pull: () => void registry.run("git.pull.open"),
            }),
          }}
        />
      )}
      {open?.kind === "pull" && deps && (
        <PullDialog
          onClose={() => setOpen(undefined)}
          deps={{ ...deps, refs: () => windowBranch().get().refs }}
        />
      )}
      {dialogs.element}
    </>
  );
}
