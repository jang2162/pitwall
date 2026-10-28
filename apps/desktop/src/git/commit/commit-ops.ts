// 커밋 툴 윈도우의 쓰기(docs/design/13-m2-plan.md M2-16 — 실행은 M2-07 · M2-08 의 RPC): 무엇을 묻고 어떤 인자로 부를지만 정한다. 모두
// 쓰기 실행기로 git CLI 가 하고, 부르는 연결은 프로젝트 창의 연결이다(Host 는 git 쓰기를 그 프로젝트 창의 연결 —
// `ConnectionScope::Project` — 로만 받는다).
//
// - 묻는 화면은 M1 의 확인 부품(file-tree/file-dialogs.tsx)이다. 작업 트리 변경 버리기 · stash 버리기는 결정 M2-D6 ① 의 확인 대상이다
//   — 위험 작업 다이얼로그(M2-20)가 들어오면 그 부품으로 바꾼다.
// - 되돌리기(⌘⌥Z): 스테이징된 것은 스테이징을 풀고(`git.unstage`) 작업 트리를 index 로 돌린다(`git.discard`) — 둘을 거치면 HEAD 다.
//   untracked · 새로 더한 파일은 지우지 않는다(`git.discard` 가 돌려준 목록을 알린다).
// - git 이 거절한 것(`2004`)은 쓰기 Task 의 실패 알림이 원문을 보였다 — 여기서 또 알리지 않는다. 커밋의 거절(훅 실패)만은 툴 윈도우에
//   그 출력을 남긴다(완료 기준 — 메시지 칸이 그대로 남는다).
// - 경로는 모두 작업 트리 꼭대기 기준 상대 경로(`/`)다(`git.status` 의 경로 그대로).

import {
  type GitCommitResult,
  type GitStashList,
  type GitStashApplyResult,
  RpcCallError,
  type RpcClient,
  type StashEntry,
} from "@pitwall/rpc-client";

import type { ConfirmQuestion, NameQuestion } from "../../file-tree/file-ops";
import type { NotificationInput } from "../../notifications/notifications";
import { type CommitItem, listLines, unstagePaths } from "./commit-model";
import type { StageSelection } from "./hunk-staging";
import type { GitCommitStrings } from "./strings";

/** `crates/pw-git/src/write/mod.rs` 의 git 실패(원문은 Task 알림이 보였다). */
export const GIT_FAILED = 2004;
/** JSON-RPC 의 conflict(그사이 바뀜). */
export const CONFLICT = -32004;
/** 확인 목록에 보이는 줄 수. */
const LISTED = 30;

export function errorText(e: unknown): string {
  if (e instanceof RpcCallError) {
    const stderr = (e.data as { stderr?: unknown } | undefined)?.stderr;
    return typeof stderr === "string" && stderr.trim() !== "" ? stderr.trim() : e.message;
  }
  return e instanceof Error ? e.message : String(e);
}

export interface CommitOpsDeps {
  client(): RpcClient | undefined;
  projectId: string;
  /** 지금 활성 워크트리(`ActiveWorktree.top`). 누른 때의 값을 읽는다. */
  worktree(): string | undefined;
  confirm(question: ConfirmQuestion): Promise<boolean>;
  askName(question: NameQuestion): Promise<string | undefined>;
  notify(input: NotificationInput): void;
  strings(): GitCommitStrings;
}

/** 커밋의 결과: 됐다 · git 이 거절했다(출력) · 그 밖의 실패. */
export type CommitOutcome =
  | { kind: "done"; result: GitCommitResult }
  | { kind: "rejected"; output: string }
  | { kind: "failed"; message: string };

export type CommitOps = ReturnType<typeof createCommitOps>;

export function createCommitOps(deps: CommitOpsDeps) {
  const T = () => deps.strings();
  const host = () => {
    const c = deps.client();
    if (!c) throw new Error("Host is not connected");
    return c;
  };
  const scope = (worktree = deps.worktree()) => ({
    projectId: deps.projectId,
    worktree: worktree ?? null,
  });
  /** 실패를 알린다(git 의 거절은 Task 알림이 이미 보였다). */
  const failed = (e: unknown) => {
    if (e instanceof RpcCallError && e.code === GIT_FAILED) return;
    deps.notify({ level: "error", title: T().result.failed(errorText(e)) });
  };
  const run = async (call: () => Promise<unknown>): Promise<boolean> => {
    try {
      await call();
      return true;
    } catch (e) {
      failed(e);
      return false;
    }
  };

  return {
    /** 파일 체크: 스테이징됨 그룹이면 스테이징을 풀고, 아니면 스테이징한다. */
    async toggle(items: readonly CommitItem[]): Promise<boolean> {
      const staged = items.filter((i) => i.group === "staged");
      const rest = items.filter((i) => i.group === "changes" || i.group === "untracked");
      return run(async () => {
        if (staged.length > 0)
          await host().call("git.unstage", { ...scope(), paths: unstagePaths(staged) });
        if (rest.length > 0)
          await host().call("git.stage", {
            ...scope(),
            paths: [...new Set(rest.map((i) => i.path))],
          });
      });
    },

    /** 변경 되돌리기(⌘⌥Z, 머리 주석). 묻고 나서 한다. 한 것이 있으면 참. */
    async rollback(items: readonly CommitItem[]): Promise<boolean> {
      const chosen = items.filter((i) => i.group === "staged" || i.group === "changes");
      if (chosen.length === 0) {
        deps.notify({ level: "warning", title: T().rollback.none });
        return false;
      }
      const paths = [...new Set(chosen.map((i) => i.path))].sort();
      const ok = await deps.confirm({
        title: T().rollback.title,
        body: `${T().rollback.body(paths.length)}\n\n${listLines(paths, LISTED, T().rollback.more)}`,
        confirm: T().rollback.confirm,
        danger: true,
      });
      if (!ok) return false;
      const staged = chosen.filter((i) => i.group === "staged");
      return run(async () => {
        if (staged.length > 0)
          await host().call("git.unstage", { ...scope(), paths: unstagePaths(staged) });
        const result = await host().call("git.discard", { ...scope(), paths, confirmed: true });
        if (result.untracked.length > 0)
          deps.notify({
            level: "info",
            title: T().rollback.untracked(listLines(result.untracked, LISTED, T().rollback.more)),
          });
      });
    },

    /** diff 미리보기의 체크(hunk-staging.ts). `worktree` 는 그 탭의 워크트리. conflict 면 거짓(다시 읽을 차례). */
    async stageLines(
      mode: "stage" | "unstage",
      path: string,
      oldPath: string | undefined,
      selection: StageSelection,
      worktree: string | undefined,
    ): Promise<"done" | "changed" | "failed"> {
      try {
        await host().call(mode === "stage" ? "git.stageLines" : "git.unstageLines", {
          ...scope(worktree),
          path,
          oldPath: mode === "unstage" ? (oldPath ?? null) : null,
          hunks: [{ hunk: selection.hunk, lines: [...selection.lines] }],
          ignoreWhitespace: false,
        });
        return "done";
      } catch (e) {
        if (e instanceof RpcCallError && e.code === CONFLICT) {
          deps.notify({ level: "warning", title: T().hunk.changed });
          return "changed";
        }
        failed(e);
        return "failed";
      }
    },

    /** diff 미리보기의 묶음 되돌리기. 묻고 나서 한다. */
    async discardLines(
      path: string,
      selection: StageSelection,
      worktree: string | undefined,
    ): Promise<"done" | "changed" | "failed" | "cancelled"> {
      const ok = await deps.confirm({
        title: T().hunk.revertTitle,
        body: `${T().hunk.revertBody}\n\n${path}`,
        confirm: T().rollback.confirm,
        danger: true,
      });
      if (!ok) return "cancelled";
      try {
        await host().call("git.discardLines", {
          ...scope(worktree),
          path,
          hunks: [{ hunk: selection.hunk, lines: [...selection.lines] }],
          ignoreWhitespace: false,
          confirmed: true,
        });
        return "done";
      } catch (e) {
        if (e instanceof RpcCallError && e.code === CONFLICT) {
          deps.notify({ level: "warning", title: T().hunk.changed });
          return "changed";
        }
        failed(e);
        return "failed";
      }
    },

    /** 커밋한다. 거절(훅 실패 등)이면 그 출력을 돌려준다 — 알리지 않는다(Task 알림이 보였다). */
    async commit(message: string, amend: boolean, signoff: boolean): Promise<CommitOutcome> {
      try {
        const result = await host().call("git.commit", { ...scope(), message, amend, signoff });
        return { kind: "done", result };
      } catch (e) {
        if (e instanceof RpcCallError && e.code === GIT_FAILED) {
          const stderr = (e.data as { stderr?: unknown } | undefined)?.stderr;
          return { kind: "rejected", output: typeof stderr === "string" ? stderr : e.message };
        }
        return { kind: "failed", message: errorText(e) };
      }
    },

    /** HEAD 커밋의 메시지(amend 가 미리 채운다). 없으면(커밋이 없는 저장소) `undefined`. */
    async headMessage(): Promise<string | undefined> {
      try {
        return (await host().call("git.show", { ...scope(), rev: "HEAD" })).message;
      } catch {
        return undefined;
      }
    },

    /** `commit.template` 의 내용. 없으면 `undefined`. */
    async template(): Promise<string | undefined> {
      try {
        return (await host().call("git.commit.template", scope())).template ?? undefined;
      } catch (e) {
        console.warn("[git] git.commit.template failed", errorText(e));
        return undefined;
      }
    },

    async stashList(): Promise<GitStashList | undefined> {
      try {
        return await host().call("git.stash.list", scope());
      } catch {
        return undefined;
      }
    },

    /** stash 를 만든다(메시지를 묻는다). 만들었으면 참. */
    async stashPush(includeUntracked: boolean): Promise<boolean> {
      const message = await deps.askName({
        title: T().stash.name,
        hint: T().stash.hint,
        initial: "",
        validate: () => undefined,
      });
      if (message === undefined) return false;
      try {
        const result = await host().call("git.stash.push", {
          ...scope(),
          message: message.trim() === "" ? null : message,
          includeUntracked,
        });
        if (!result.stash) deps.notify({ level: "info", title: T().stash.nothing });
        return result.stash !== null;
      } catch (e) {
        failed(e);
        return false;
      }
    },

    /** stash 를 적용한다(`pop` 이면 지운다). */
    async stashApply(stash: StashEntry, pop: boolean): Promise<boolean> {
      try {
        const result: GitStashApplyResult = await host().call(
          pop ? "git.stash.pop" : "git.stash.apply",
          { ...scope(), index: stash.index, commit: stash.commit, reinstateIndex: false },
        );
        if (result.conflicted) deps.notify({ level: "warning", title: T().stash.conflicted });
        return true;
      } catch (e) {
        if (e instanceof RpcCallError && e.code === CONFLICT)
          deps.notify({ level: "warning", title: T().stash.changed });
        else failed(e);
        return false;
      }
    },

    /** stash 를 버린다(묻는다). */
    async stashDrop(stash: StashEntry): Promise<boolean> {
      const ok = await deps.confirm({
        title: T().stash.dropTitle,
        body: T().stash.dropBody(`stash@{${stash.index}} ${stash.message}`),
        confirm: T().stash.drop,
        danger: true,
      });
      if (!ok) return false;
      try {
        await host().call("git.stash.drop", {
          ...scope(),
          index: stash.index,
          commit: stash.commit,
          confirmed: true,
        });
        return true;
      } catch (e) {
        if (e instanceof RpcCallError && e.code === CONFLICT)
          deps.notify({ level: "warning", title: T().stash.changed });
        else failed(e);
        return false;
      }
    },

    /** 작업 트리 상태를 처음부터 다시 읽는다(⟳). */
    async status() {
      return host().call("git.status", { ...scope(), paths: null });
    },
  };
}
