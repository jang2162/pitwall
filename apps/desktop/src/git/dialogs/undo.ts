// 작업 결과의 [되돌리기](13-m2-plan.md M2-20 · 「공유 부품의 주인」 의 `GitRefMove` 행, 결정 M2-D6 ③). ref 를 옮긴 쓰기의 결과
// `moved { refName, previous, current }` 로 되돌린다:
//
// - HEAD 의 ref(브랜치 또는 분리된 HEAD): `git.reset { commit: previous, mode, expected: current, expectedRef: refName }`. mode 는 soft
//   reset · amend 를 되돌리면 soft, mixed reset 이면 mixed, 그 밖(hard reset · merge · rebase · cherry-pick · revert)은 keep. soft 가
//   아니면 먼저 `git.resetPreview`(같은 mode)로 잃을 것을 보고(결정 M2-D6 ① 「되돌리기도 미리보기를 거친다」), 잃을 것이 있으면 그
//   목록으로 묻고 그 지문을 실어 부른다. 지문이 `null` 이면 「확인할 수 없음」 으로 하지 않는다. 그사이 작업 트리가 바뀌어 지문이
//   다르면 Host 가 conflict 로 새 목록을 주고, 한 번 더 묻는다. 그사이 HEAD 커밋 · HEAD 의 ref 가 바뀌었으면 Host 의 거절(conflict)을
//   그대로 오류 알림으로 보인다 — 아무것도 하지 않는다.
// - 지운 브랜치(`current: null`): `previous` 에 같은 이름으로 다시 만든다(체크아웃하지 않는다).
// - 만든 브랜치(`previous: null`): 그 브랜치를 지운다 — 그사이 움직였으면(지금 끝이 `current` 가 아니면) 그 사실을 알리고 지우지
//   않는다. 병합 안 됐다고 git 이 거절하면(2101) 원문과 지금 끝을 보이고 「강제 삭제」 를 물은 뒤에만.
//
// 부르는 연결은 프로젝트 창의 연결이다(Host 는 git 쓰기를 그 프로젝트 창의 연결로만 받는다).

import {
  ERROR_CODE,
  type GitLostChanges,
  type GitRefMove,
  RpcCallError,
  type RpcClient,
} from "@pitwall/rpc-client";

import type { NotificationInput } from "../../notifications/notifications";
import type { GitConfirm } from "./git-dialogs";
import type { GitDialogStrings } from "./strings";

/** `crates/pw-git/src/branch/mod.rs` 의 `code::NOT_MERGED`. */
export const NOT_MERGED = 2101;
/** `crates/pw-git/src/write/mod.rs` 의 git 실패(원문은 Task 알림이 이미 보였다). */
export const GIT_FAILED = 2004;

export type UndoMode = "soft" | "mixed" | "keep";

/** 무엇을 되돌리는가. */
export type UndoKind =
  | { kind: "head"; mode: UndoMode }
  | { kind: "branchDeleted" }
  | { kind: "branchCreated" };

export interface UndoDeps {
  client: RpcClient;
  projectId: string;
  /** 활성 워크트리(`ActiveWorktree.top`). */
  worktree?: string;
  confirm(question: GitConfirm): Promise<boolean>;
  notify(input: NotificationInput): void;
  strings: GitDialogStrings;
}

export function shortHash(hash: string | null): string {
  return hash ? hash.slice(0, 8) : "∅";
}

/** `refs/heads/x` → `x`. */
export function branchOf(refName: string): string {
  return refName.replace(/^refs\/heads\//, "");
}

/** 오류의 글: git 의 원문(`data.stderr`)이 있으면 그것. */
export function errorText(e: unknown): string {
  if (e instanceof RpcCallError) {
    const stderr = (e.data as { stderr?: unknown } | null | undefined)?.stderr;
    return typeof stderr === "string" && stderr.trim() !== "" ? stderr.trim() : e.message;
  }
  return e instanceof Error ? e.message : String(e);
}

/** conflict 의 `data`(reset 이 지문이 다를 때 싣는 새 목록). */
interface ResetConflict {
  head?: string | null;
  lost?: GitLostChanges;
  fingerprint?: string | null;
}

export function resetConflictOf(e: unknown): ResetConflict | undefined {
  if (!(e instanceof RpcCallError) || e.code !== ERROR_CODE.CONFLICT) return undefined;
  return (e.data ?? {}) as ResetConflict;
}

/** 이 이동에 [되돌리기] 를 달 수 있는가. */
export function canUndo(move: GitRefMove, kind: UndoKind): boolean {
  switch (kind.kind) {
    case "head":
      return move.previous !== null;
    case "branchDeleted":
      return move.previous !== null && move.refName.startsWith("refs/heads/");
    case "branchCreated":
      return move.refName.startsWith("refs/heads/");
  }
}

/** 되돌린다(머리 주석). 되돌렸으면 참. 실패 · 취소는 거짓(실패는 알렸다). */
export async function undoMove(deps: UndoDeps, move: GitRefMove, kind: UndoKind): Promise<boolean> {
  const T = deps.strings;
  const base = { projectId: deps.projectId, worktree: deps.worktree ?? null };
  const fail = (e: unknown) => {
    deps.notify({ level: "error", title: T.undo.failed, body: errorText(e) });
    return false;
  };
  if (!canUndo(move, kind)) return false;
  const name = branchOf(move.refName);
  try {
    if (kind.kind === "branchDeleted") {
      await deps.client.call("git.branch.create", {
        ...base,
        name,
        start: move.previous,
        checkout: false,
      });
      deps.notify({ level: "info", title: T.undo.restored(name) });
      return true;
    }
    if (kind.kind === "branchCreated") {
      // 그사이 움직였으면(커밋을 쌓았으면) 지우지 않는다 — 기대 해시를 받는 삭제가 없으므로 지금 끝을 먼저 읽고, 병합 안 됨
      // (2101)의 `data.commit` 으로 한 번 더 본다.
      const refused = (now: string | null) => {
        deps.notify({
          level: "error",
          title: T.undo.failed,
          body: T.undo.movedSince(name, shortHash(now)),
        });
        return false;
      };
      const refs = await deps.client.call("git.refs", base);
      const now = refs.branches.find((b) => b.refName === move.refName)?.commit ?? null;
      if (now !== move.current) return refused(now);
      const remove = (force: boolean) =>
        deps.client.call("git.branch.delete", { ...base, name, force });
      try {
        await remove(false);
      } catch (e) {
        if (!(e instanceof RpcCallError && e.code === NOT_MERGED)) throw e;
        const data = (e.data ?? {}) as { commit?: unknown };
        const tip = typeof data.commit === "string" ? data.commit : null;
        if (tip !== move.current) return refused(tip);
        const ok = await deps.confirm({
          title: T.branch.forceTitle(name),
          body: T.branch.forceBody(name, shortHash(tip)),
          detail: errorText(e),
          confirm: T.branch.forceConfirm,
          danger: true,
        });
        if (!ok) return false;
        await remove(true);
      }
      deps.notify({ level: "info", title: T.undo.removed(name) });
      return true;
    }
  } catch (e) {
    return fail(e);
  }

  // HEAD 의 ref.
  const previous = move.previous!;
  const mode = kind.mode;
  const reset = (fingerprint: string | null) =>
    deps.client.call("git.reset", {
      ...base,
      commit: previous,
      mode,
      expected: move.current,
      expectedRef: move.refName,
      fingerprint,
    });
  const title = T.undo.title(shortHash(previous));
  /** 잃을 것을 보이고 묻는다. 확인할 수 없으면 그 표시만 보이고 거짓. */
  const ask = async (lost: GitLostChanges, fingerprint: string | null, changed: boolean) => {
    const count = lost.changes.length + lost.overwritten.length;
    if (fingerprint !== null && count === 0) return true;
    const ok = await deps.confirm({
      title,
      ...(changed && { body: T.lost.changed }),
      lost: { changes: lost, fingerprint },
      confirm: T.undo.confirm,
      danger: true,
    });
    // 확인할 수 없는 목록이면 확인 단추가 꺼져 있다 — 어떤 답이든 하지 않는다.
    return ok && fingerprint !== null;
  };
  try {
    let fingerprint: string | null = null;
    if (mode !== "soft") {
      const preview = await deps.client.call("git.resetPreview", {
        ...base,
        commit: previous,
        mode,
      });
      if (!(await ask(preview.lost, preview.fingerprint, false))) return false;
      fingerprint = preview.fingerprint;
    }
    try {
      await reset(fingerprint);
    } catch (e) {
      const conflict = resetConflictOf(e);
      // HEAD 가 그사이 움직였으면 새 목록이 없다 — 그 거절을 그대로 보인다.
      if (!conflict?.lost) throw e;
      const next = conflict.fingerprint ?? null;
      if (!(await ask(conflict.lost, next, true))) return false;
      await reset(next);
    }
    deps.notify({ level: "info", title: T.undo.done(name) });
    return true;
  } catch (e) {
    return fail(e);
  }
}

/** 이동을 알리고 그 알림에 [되돌리기] 를 단다(되돌릴 수 없는 이동이면 단추 없이). */
export function offerUndo(deps: UndoDeps, move: GitRefMove, kind: UndoKind, title?: string): void {
  const T = deps.strings;
  deps.notify({
    level: "info",
    title:
      title ??
      T.undo.moved(branchOf(move.refName), shortHash(move.previous), shortHash(move.current)),
    ...(canUndo(move, kind) && {
      action: { label: T.undo.button, run: () => void undoMove(deps, move, kind) },
    }),
  });
}
