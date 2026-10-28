// 브랜치 팝업 · Search Everywhere 의 브랜치 탭이 부르는 동작(13-m2-plan.md M2-13). 모두 M2-09 · M2-10 의 쓰기 RPC 로 git CLI 가 하고
// (쓰기 실행기, 그 프로젝트 창의 연결만 통과한다), 이 모듈은 무엇을 묻고 어떤 인자로 부를지만 정한다.
//
// - 체크아웃: 로컬 브랜치는 그대로, 원격 브랜치는 그것을 upstream 으로 하는 로컬 브랜치를 만들어 간다(`git.checkout` 의 `remote`).
//   같은 이름의 로컬 브랜치가 이미 그 원격을 따르면 그 로컬 브랜치로 간다. 다른 것을 따르면(또는 아무것도 안 따르면) 새 로컬 이름을
//   묻는다. 태그는 분리된 HEAD 다. 미저장 문서와 겹치는 확인은 쓰기 실행기가 창에 묻는다(git-prompts.tsx).
// - 삭제: 먼저 `force` 없이 부른다. 병합 안 된 브랜치면 Host 가 `2101` 로 거절하고, 그때만 git 의 원문과 함께 「강제 삭제」 를 위험
//   다이얼로그로 묻는다(결정 M2-D6 ①). 지운 결과의 이전 해시를 알림에 적고 [되돌리기](그 해시에 다시 만든다)를 단다(M2-20).
// - 새 브랜치 · 이름 바꾸기는 브랜치 이름 다이얼로그(M2-20, dialogs/git-dialogs.tsx)로 묻는다. 만든 브랜치의 알림에도 [되돌리기]
//   (지운다)를 단다.
// - git 에 넘기는 리비전(merge · rebase · 태그 체크아웃 · 새 브랜치의 시작 · 비교)은 ref 전체 이름(`refs/heads/` · `refs/remotes/` ·
//   `refs/tags/`)이다 — 같은 이름의 브랜치와 태그가 있어도 고른 것으로 풀린다. merge · rebase 는 Host 가 실행 직전에 모호하지
//   않은 짧은 이름으로 바꿔 git 을 부른다(메시지 · reflog 가 터미널과 같다). 로컬 브랜치의 체크아웃 · 이름 바꾸기 · 삭제는 RPC 가
//   짧은 이름을 받아 Host 가 `refs/heads/<이름>` 으로만 다룬다(crates/pw-git/src/branch/mod.rs — `git switch` · `git branch` 는
//   브랜치만 본다). 원격 체크아웃도 Host 가 `refs/remotes/<원격>/<브랜치>` 를 만든다.
// - merge · rebase 는 확인 한 번을 묻고(위험 표시는 없다 — 결정 M2-D6 ① 의 목록 밖), ref 를 옮겼으면 그 알림에 [되돌리기]
//   (`reset --keep`, 미리보기를 거친다 — dialogs/undo.ts)를 단다.
// - git 이 거절한 것(`2004`)은 쓰기 Task 의 실패 알림이 이미 원문을 보였다 — 여기서 또 알리지 않는다. 그 밖의 실패는 알린다.

import type { ActionRegistry } from "@pitwall/keymap";
import {
  type GitBranch,
  type GitRefMove,
  type GitRefs,
  type GitRemoteBranch,
  type GitTag,
  RpcCallError,
  type RpcClient,
} from "@pitwall/rpc-client";

import type { NameQuestion } from "../file-tree/file-ops";
import type { NotificationInput } from "../notifications/notifications";
import type { BranchStrings } from "./branch-strings";
import { SHORT_HASH, headBranch, headName } from "./branch-status";
import { compareCommits } from "./compare-action";
import type { BranchNameAnswer, BranchNameQuestion, GitConfirm } from "./dialogs/git-dialogs";
import type { GitDialogStrings } from "./dialogs/strings";
import { GIT_FAILED, NOT_MERGED, type UndoKind, shortHash } from "./dialogs/undo";

export { GIT_FAILED, NOT_MERGED };

/** 팝업 · 검색이 고르는 ref 하나. */
export type BranchRef =
  | { readonly kind: "local"; readonly branch: GitBranch }
  | { readonly kind: "remote"; readonly branch: GitRemoteBranch }
  | { readonly kind: "tag"; readonly tag: GitTag };

export function refName(ref: BranchRef): string {
  return ref.kind === "tag" ? ref.tag.refName : ref.branch.refName;
}

/** 화면에 보이는 짧은 이름(`main`, `origin/main`, `v1.0`). */
export function shortName(ref: BranchRef): string {
  return ref.kind === "tag" ? ref.tag.name : ref.branch.name;
}

/** `refs` 의 모든 ref(로컬 → 원격 → 태그). */
export function allRefs(refs: GitRefs): BranchRef[] {
  return [
    ...refs.branches.map((branch): BranchRef => ({ kind: "local", branch })),
    ...refs.remoteBranches.map((branch): BranchRef => ({ kind: "remote", branch })),
    ...refs.tags.map((tag): BranchRef => ({ kind: "tag", tag })),
  ];
}

/** 원격 브랜치 `origin/feat/x` 의 원격 안 이름(`feat/x`). */
export function remoteBranchName(branch: GitRemoteBranch): string {
  const prefix = `${branch.remote}/`;
  return branch.name.startsWith(prefix) ? branch.name.slice(prefix.length) : branch.name;
}

/**
 * 새 브랜치 이름으로 쓸 수 없으면 그 이유(git `check-ref-format` 의 흔한 규칙만 — 나머지는 git 이 원문으로 거절한다). `taken` 은
 * 이미 있는 로컬 브랜치 이름.
 */
export function branchNameProblem(
  T: BranchStrings,
  name: string,
  taken: ReadonlySet<string>,
): string | undefined {
  if (name.trim() === "") return T.dialogs.emptyName;
  if (
    /[\s~^:?*[\\]/.test(name) ||
    [...name].some((c) => c.charCodeAt(0) < 0x20 || c.charCodeAt(0) === 0x7f) ||
    name.startsWith("-") ||
    name.startsWith("/") ||
    name.endsWith("/") ||
    name.endsWith(".") ||
    name.endsWith(".lock") ||
    name.includes("..") ||
    name.includes("//") ||
    name.includes("@{") ||
    name === "@"
  )
    return T.dialogs.badName;
  if (taken.has(name)) return T.dialogs.badName;
  return undefined;
}

export interface BranchOpsDeps {
  client(): RpcClient | undefined;
  projectId: string;
  /** 지금 활성 워크트리(`ActiveWorktree.top`). 누른 때의 값을 읽는다. */
  worktree(): string | undefined;
  /** 지금 ref(창의 `windowBranch()`). */
  refs(): GitRefs | undefined;
  askName(question: NameQuestion): Promise<string | undefined>;
  /** 브랜치 이름 다이얼로그(M2-20). */
  askBranch(question: BranchNameQuestion): Promise<BranchNameAnswer | undefined>;
  /** 위험 작업 다이얼로그(M2-20). */
  confirm(question: GitConfirm): Promise<boolean>;
  notify(input: NotificationInput): void;
  /** ref 를 옮긴 결과를 알리고 [되돌리기] 를 단다(dialogs/undo.ts 의 `offerUndo`). */
  offerUndo(move: GitRefMove, kind: UndoKind, title: string): void;
  strings(): BranchStrings;
  dialogStrings(): GitDialogStrings;
  registry: ActionRegistry;
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

export function createBranchOps(deps: BranchOpsDeps) {
  const scope = () => ({ projectId: deps.projectId, worktree: deps.worktree() ?? null });
  const host = () => {
    const c = deps.client();
    if (!c) throw new Error("Host is not connected");
    return c;
  };
  const taken = () => new Set((deps.refs()?.branches ?? []).map((b) => b.name));
  const current = () => {
    const refs = deps.refs();
    return (refs && headName(refs)) ?? "HEAD";
  };

  /** 실패를 알린다 — git 의 거절(Task 가 이미 알렸다)은 빼고. */
  const failed = (what: string) => (e: unknown) => {
    if (e instanceof RpcCallError && e.code === GIT_FAILED) return;
    deps.notify({ level: "error", title: deps.strings().results.failed(what), body: errorText(e) });
  };

  const info = (title: string, body?: string) =>
    deps.notify({ level: "info", title, ...(body !== undefined && { body }) });

  /** merge · rebase 의 결과: ref 를 옮겼으면 [되돌리기](keep)와 함께 알린다. */
  const settled = (result: { outcome: string; moved: GitRefMove | null }, done: string) => {
    const T = deps.strings();
    if (result.outcome === "upToDate") return info(T.results.upToDate);
    if (result.outcome === "stopped") return info(T.results.stopped);
    if (result.moved) deps.offerUndo(result.moved, { kind: "head", mode: "keep" }, done);
    else info(done);
  };

  async function checkout(ref: BranchRef): Promise<void> {
    const T = deps.strings();
    const what = T.popup.menu.checkout;
    try {
      if (ref.kind === "local") {
        if (ref.branch.head) return;
        await host().call("git.checkout", {
          ...scope(),
          target: { kind: "branch", name: ref.branch.name },
        });
        info(T.results.checkedOut(ref.branch.name));
        return;
      }
      if (ref.kind === "tag") {
        await host().call("git.checkout", {
          ...scope(),
          target: { kind: "commit", rev: ref.tag.refName },
        });
        info(T.results.checkedOut(ref.tag.name));
        return;
      }
      const remote = ref.branch;
      const wanted = remoteBranchName(remote);
      const existing = deps.refs()?.branches.find((b) => b.name === wanted);
      if (existing?.upstream?.refName === remote.refName) {
        if (!existing.head)
          await host().call("git.checkout", {
            ...scope(),
            target: { kind: "branch", name: existing.name },
          });
        info(T.results.checkedOut(existing.name));
        return;
      }
      let local = wanted;
      if (existing) {
        const names = taken();
        const asked = await deps.askName({
          title: T.popup.menu.checkoutTracking(remote.name),
          hint: T.dialogs.newHint(remote.name),
          initial: wanted,
          validate: (name) => branchNameProblem(T, name, names),
        });
        if (asked === undefined) return;
        local = asked;
      }
      await host().call("git.checkout", {
        ...scope(),
        target: {
          kind: "remote",
          remote: remote.remote,
          branch: wanted,
          ...(local !== wanted && { local }),
        },
      });
      info(T.results.checkedOut(local));
    } catch (e) {
      failed(what)(e);
    }
  }

  /** 새 브랜치를 만든다(「만든 뒤 체크아웃」 이 처음부터 켜져 있다). `start` 가 없으면 HEAD 에서. */
  async function create(start?: BranchRef): Promise<void> {
    const T = deps.strings();
    const D = deps.dialogStrings();
    const from = start ? shortName(start) : current();
    const answer = await deps.askBranch({
      title: D.branch.createTitle,
      hint: D.branch.createFrom(from),
      initial: start?.kind === "remote" ? remoteBranchName(start.branch) : "",
      confirm: D.branch.create,
      taken: taken(),
      checkout: true,
    });
    if (answer === undefined) return;
    try {
      const result = await host().call("git.branch.create", {
        ...scope(),
        name: answer.name,
        ...(start && { start: refName(start) }),
        checkout: answer.checkout,
      });
      deps.offerUndo(
        { refName: result.refName, previous: null, current: result.commit },
        { kind: "branchCreated" },
        T.results.created(answer.name),
      );
    } catch (e) {
      failed(T.popup.newBranch)(e);
    }
  }

  async function rename(branch: GitBranch): Promise<void> {
    const T = deps.strings();
    const D = deps.dialogStrings();
    const names = taken();
    names.delete(branch.name);
    const answer = await deps.askBranch({
      title: D.branch.renameTitle(branch.name),
      initial: branch.name,
      confirm: D.branch.rename,
      taken: names,
    });
    if (answer === undefined || answer.name === branch.name) return;
    try {
      await host().call("git.branch.rename", {
        ...scope(),
        name: branch.name,
        newName: answer.name,
      });
      info(T.results.renamed(answer.name));
    } catch (e) {
      failed(T.popup.menu.rename)(e);
    }
  }

  async function remove(branch: GitBranch): Promise<void> {
    const T = deps.strings();
    const call = (force: boolean) =>
      host().call("git.branch.delete", { ...scope(), name: branch.name, force });
    try {
      let result;
      try {
        result = await call(false);
      } catch (e) {
        if (!(e instanceof RpcCallError && e.code === NOT_MERGED)) throw e;
        const stderr =
          e.data &&
          typeof e.data === "object" &&
          !Array.isArray(e.data) &&
          typeof e.data.stderr === "string"
            ? e.data.stderr.trim()
            : "";
        const D = deps.dialogStrings();
        const commit =
          e.data && typeof e.data === "object" && !Array.isArray(e.data)
            ? (e.data as { commit?: unknown }).commit
            : undefined;
        const ok = await deps.confirm({
          title: D.branch.forceTitle(branch.name),
          body: D.branch.forceBody(
            branch.name,
            shortHash(typeof commit === "string" ? commit : branch.commit),
          ),
          ...(stderr && { detail: stderr }),
          confirm: D.branch.forceConfirm,
          danger: true,
        });
        if (!ok) return;
        result = await call(true);
      }
      const previous = result.deleted.previous;
      deps.offerUndo(
        result.deleted,
        { kind: "branchDeleted" },
        T.results.deleted(branch.name, previous ? previous.slice(0, SHORT_HASH) : ""),
      );
    } catch (e) {
      failed(T.popup.menu.delete)(e);
    }
  }

  async function merge(ref: BranchRef): Promise<void> {
    const T = deps.strings();
    const name = shortName(ref);
    const ok = await deps.confirm({
      title: T.dialogs.mergeTitle(name, current()),
      body: T.dialogs.historyBody,
      confirm: T.dialogs.proceed,
    });
    if (!ok) return;
    try {
      // 전체 이름으로 보낸다 — 짧은 이름은 같은 이름의 태그 · 다른 종류의 ref 로 풀릴 수 있다(git 은 경고만 하고 태그를 merge
      // 한다). Host 가 잠금 안에서 짧은 이름이 모호하지 않으면 짧은 이름으로 git 을 불러 merge 메시지가 터미널의 `git merge x` 와
      // 같다(crates/pw-git/src/history/mod.rs `unambiguous_short`).
      const result = await host().call("git.merge", { ...scope(), rev: refName(ref) });
      settled(result, T.results.merged(name));
    } catch (e) {
      failed(T.popup.menu.merge(name, current()))(e);
    }
  }

  async function rebase(ref: BranchRef): Promise<void> {
    const T = deps.strings();
    const name = shortName(ref);
    const ok = await deps.confirm({
      title: T.dialogs.rebaseTitle(current(), name),
      body: T.dialogs.historyBody,
      confirm: T.dialogs.proceed,
    });
    if (!ok) return;
    try {
      const result = await host().call("git.rebase", { ...scope(), upstream: refName(ref) });
      settled(result, T.results.rebased(current()));
    } catch (e) {
      failed(T.popup.menu.rebase(current(), name))(e);
    }
  }

  /** 지금 HEAD 와 견준다(M2-15 의 비교 화면). */
  function compare(ref: BranchRef): void {
    const refs = deps.refs();
    const head = refs && headBranch(refs);
    const a = head?.refName ?? (refs?.head?.kind === "detached" ? refs.head.commit : "HEAD");
    if (!compareCommits(deps.registry, { a, b: refName(ref) }))
      deps.notify({ level: "warning", title: deps.strings().results.compareMissing });
  }

  return { checkout, create, rename, remove, merge, rebase, compare };
}

export type BranchOps = ReturnType<typeof createBranchOps>;
