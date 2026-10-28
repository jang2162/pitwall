// Git 로그의 쓰기(docs/design/13-m2-plan.md M2-14 의 우클릭 · 라벨 끌어 놓기 — 실행은 M2-09 · M2-10 RPC). 무엇을 묻고 답대로 무엇을
// 부르는지를 여기서 정하고, 묻는 화면은 위험 작업 다이얼로그 부품(M2-20, ../dialogs/git-dialogs.tsx — 확인 · 브랜치 이름)과 태그
// 이름만 M1 의 이름 묻기(file-tree/file-dialogs.tsx)다. 부르는 연결은 프로젝트 창의 연결이다 — Host 는 git 쓰기를 그 프로젝트 창의
// 연결(`ConnectionScope::Project`)로만 받는다.
//
// - reset: soft 는 잃을 것이 없어 묻지 않는다. hard 는 늘, mixed · keep 은 잃을 것이 있을 때만 `git.resetPreview` 의 잃을 변경을
//   보이고 확인한 뒤 그 지문(`fingerprint`)과 `expected`(미리보기 때의 HEAD)를 실어 부른다(결정 M2-D6 ①). 지문이 `null` 이면 「확인할
//   수 없음」 을 보이고 하지 않는다. 그사이 바뀌어 conflict(`-32004`)가 오면 새 목록으로 한 번 더 묻는다.
// - ref 를 옮긴 결과(`GitRefMove`)가 오면 [되돌리기] 를 단다(`moved` 에 되돌릴 mode 와 함께 — soft reset 은 soft, mixed reset 은
//   mixed, 그 밖은 keep). 되돌리기는 미리보기를 거친다(../dialogs/undo.ts).
// - git 에 리비전으로 넘기는 ref(merge · rebase 의 위 · 비교)는 언제나 전체 이름(`refFullName`)이다 — 짧은 이름이면 같은 이름의
//   태그가 브랜치보다 먼저 풀린다. merge · rebase 는 Host 가 실행 직전에 모호하지 않으면 짧은 이름으로 바꿔 메시지를 터미널과
//   맞춘다(pw-git `history` 의 `unambiguous_short`). rebase 할 브랜치(`branch`)와 체크아웃은 Host 가 브랜치로만 다뤄 짧은 이름이다.
// - cherry-pick 은 고른 커밋을 오래된 것부터, revert 는 새것부터 넘긴다(새것부터 되돌려야 뒤 커밋이 앞 커밋의 변경 위에서 풀린다).
// - 충돌로 멈추면(`outcome: stopped`) 경고 알림만 — 계속 · 중단은 커밋 툴 윈도우(M2-11 · M2-16)의 몫이다. git 이 거절하면 그
//   원문을 오류 알림의 본문에 둔다.

import type { GitLostChanges, GitRefMove, LogRef, ResetMode, RpcClient } from "@pitwall/rpc-client";

import type { NameQuestion } from "../../file-tree/file-ops";
import type { NotificationInput } from "../../notifications/notifications";
import type { BranchNameAnswer, BranchNameQuestion, GitConfirm } from "../dialogs/git-dialogs";
import type { GitDialogStrings } from "../dialogs/strings";
import {
  type UndoKind,
  type UndoMode,
  errorText,
  resetConflictOf,
  undoMove,
} from "../dialogs/undo";
import type { GitLogStrings } from "./strings";

export interface LogOpsDeps {
  client: RpcClient;
  projectId: string;
  worktree?: string;
  askName(question: NameQuestion): Promise<string | undefined>;
  askBranch(question: BranchNameQuestion): Promise<BranchNameAnswer | undefined>;
  confirm(question: GitConfirm): Promise<boolean>;
  notify(input: NotificationInput): void;
  strings: GitLogStrings;
  dialogStrings: GitDialogStrings;
  /** ref 가 움직였다 — [되돌리기] 를 단다. `kind` 가 되돌리는 법이다. */
  moved(move: GitRefMove, kind: UndoKind): void;
}

/** reset 의 mode 를 되돌리는 mode(13-m2-plan 「공유 부품의 주인」 의 `GitRefMove` 행). */
export function undoModeOf(mode: ResetMode): UndoMode {
  return mode === "soft" ? "soft" : mode === "mixed" ? "mixed" : "keep";
}

const KEEP: UndoKind = { kind: "head", mode: "keep" };

export function short(hash: string): string {
  return hash.slice(0, 8);
}

/** 로그 라벨의 ref 전체 이름(분리된 HEAD 는 `HEAD`). */
export function refFullName(ref: LogRef): string {
  switch (ref.kind) {
    case "head":
      return "HEAD";
    case "branch":
      return `refs/heads/${ref.name}`;
    case "remote":
      return `refs/remotes/${ref.name}`;
    case "tag":
      return `refs/tags/${ref.name}`;
  }
}

/**
 * 브랜치 · 태그 이름을 미리 거른다(Host 가 `git check-ref-format --branch` 규칙으로 다시 본다 — 여기는 빈 이름과 흔한 실수만).
 * 쓸 수 없으면 그 이유.
 */
export function refNameProblem(name: string, strings: GitLogStrings["dialog"]): string | undefined {
  if (name.trim() === "") return strings.nameEmpty;
  if (
    name.startsWith("-") ||
    name.startsWith("/") ||
    name.endsWith("/") ||
    name.endsWith(".") ||
    name.endsWith(".lock") ||
    name === "HEAD" ||
    name === "@" ||
    name.includes("..") ||
    name.includes("@{") ||
    name.includes("//") ||
    /[\s~^:?*[\\]/.test(name) ||
    [...name].some((c) => c.charCodeAt(0) < 0x20 || c.charCodeAt(0) === 0x7f)
  )
    return strings.nameInvalid;
  return undefined;
}

export class LogOps {
  constructor(private readonly deps: LogOpsDeps) {}

  private get base() {
    return { projectId: this.deps.projectId, worktree: this.deps.worktree ?? null };
  }

  /** 쓰기 하나를 돌리고 실패를 알린다. 결과의 `moved` 가 있으면 [되돌리기]. */
  private async run<T>(action: string, call: () => Promise<T>): Promise<T | undefined> {
    try {
      return await call();
    } catch (e) {
      this.deps.notify({
        level: "error",
        title: this.deps.strings.result.failed(action),
        body: errorText(e),
      });
      return undefined;
    }
  }

  private history(result: { outcome: string; moved: GitRefMove | null } | undefined): void {
    if (!result) return;
    const T = this.deps.strings.result;
    if (result.outcome === "stopped") {
      this.deps.notify({ level: "warning", title: T.stopped, body: T.stoppedBody });
      return;
    }
    if (result.outcome === "upToDate") {
      this.deps.notify({ level: "info", title: T.upToDate });
      return;
    }
    if (result.moved) this.deps.moved(result.moved, KEEP);
  }

  async checkoutBranch(name: string, label: string) {
    await this.run(label, () =>
      this.deps.client.call("git.checkout", { ...this.base, target: { kind: "branch", name } }),
    );
  }

  /** 원격 브랜치(`origin/feat`) → 추적 브랜치를 만들어 체크아웃. */
  async checkoutRemote(remoteName: string, label: string) {
    const at = remoteName.indexOf("/");
    if (at <= 0) return;
    const remote = remoteName.slice(0, at);
    const branch = remoteName.slice(at + 1);
    await this.run(label, () =>
      this.deps.client.call("git.checkout", {
        ...this.base,
        target: { kind: "remote", remote, branch },
      }),
    );
  }

  async checkoutCommit(hash: string, label: string) {
    await this.run(label, () =>
      this.deps.client.call("git.checkout", {
        ...this.base,
        target: { kind: "commit", rev: hash },
      }),
    );
  }

  async createBranch(hash: string, label: string) {
    const D = this.deps.dialogStrings.branch;
    const answer = await this.deps.askBranch({
      title: D.createTitle,
      hint: D.createFrom(short(hash)),
      initial: "",
      confirm: D.create,
      taken: new Set(),
      checkout: false,
    });
    if (answer === undefined) return;
    await this.run(label, () =>
      this.deps.client.call("git.branch.create", {
        ...this.base,
        name: answer.name,
        start: hash,
        ...(answer.checkout && { checkout: true }),
      }),
    );
  }

  async createTag(hash: string, label: string) {
    const T = this.deps.strings.dialog;
    const name = await this.deps.askName({
      title: T.newTag,
      hint: T.from(short(hash)),
      initial: "",
      validate: (n) => refNameProblem(n, T),
    });
    if (name === undefined) return;
    await this.run(label, () =>
      this.deps.client.call("git.tag.create", { ...this.base, name, rev: hash }),
    );
  }

  /** `commits` 는 로그 차례(새것부터)다. */
  async cherryPick(commits: readonly string[], label: string) {
    const oldestFirst = [...commits].reverse();
    this.history(
      await this.run(label, () =>
        this.deps.client.call("git.cherryPick", { ...this.base, commits: oldestFirst }),
      ),
    );
  }

  /** `commits` 는 로그 차례(새것부터)다. 새것부터 되돌린다. */
  async revert(commits: readonly string[], label: string) {
    this.history(
      await this.run(label, () =>
        this.deps.client.call("git.revert", { ...this.base, commits: [...commits] }),
      ),
    );
  }

  async merge(rev: string, label: string) {
    this.history(
      await this.run(label, () => this.deps.client.call("git.merge", { ...this.base, rev })),
    );
  }

  async rebase(branch: string, upstream: string, label: string) {
    this.history(
      await this.run(label, () =>
        this.deps.client.call("git.rebase", { ...this.base, upstream, branch }),
      ),
    );
  }

  /** 잃을 것을 보이고 묻는다. 확인할 수 없으면(지문 `null`) 그 표시만 보이고 거짓. */
  private async confirmLost(
    title: string,
    lost: GitLostChanges,
    fingerprint: string | null,
    always: boolean,
    changed: boolean,
  ): Promise<boolean> {
    const D = this.deps.dialogStrings;
    const count = lost.changes.length + lost.overwritten.length;
    if (fingerprint !== null && count === 0 && !always) return true;
    const ok = await this.deps.confirm({
      title,
      ...(changed && { body: D.lost.changed }),
      lost: { changes: lost, fingerprint },
      confirm: D.reset.confirm,
      danger: always || count > 0,
    });
    // 확인할 수 없는 목록이면 확인 단추가 꺼져 있다 — 어떤 답이든 하지 않는다.
    return ok && fingerprint !== null;
  }

  /** 이 커밋으로 reset(머리 주석). */
  async reset(hash: string, mode: ResetMode, label: string) {
    const kind: UndoKind = { kind: "head", mode: undoModeOf(mode) };
    if (mode === "soft") {
      const result = await this.run(label, () =>
        this.deps.client.call("git.reset", { ...this.base, commit: hash, mode }),
      );
      if (result?.moved) this.deps.moved(result.moved, kind);
      return;
    }
    const preview = await this.run(label, () =>
      this.deps.client.call("git.resetPreview", { ...this.base, commit: hash, mode }),
    );
    if (!preview) return;
    const title = this.deps.dialogStrings.reset.title(mode, short(hash));
    let { head, lost, fingerprint } = preview;
    // 한 번 다시 묻는다(그사이 작업 트리가 바뀌었으면). 그 뒤에도 바뀌면 오류로 알린다.
    for (let attempt = 0; attempt < 2; attempt++) {
      if (!(await this.confirmLost(title, lost, fingerprint, mode === "hard", attempt > 0))) return;
      try {
        const result = await this.deps.client.call("git.reset", {
          ...this.base,
          commit: hash,
          mode,
          expected: head,
          fingerprint,
        });
        if (result.moved) this.deps.moved(result.moved, kind);
        return;
      } catch (e) {
        const conflict = resetConflictOf(e);
        if (!conflict?.lost || attempt > 0) {
          this.deps.notify({
            level: "error",
            title: this.deps.strings.result.failed(label),
            body: errorText(e),
          });
          return;
        }
        head = conflict.head ?? head;
        lost = conflict.lost;
        fingerprint = conflict.fingerprint ?? null;
      }
    }
  }

  /** [되돌리기](../dialogs/undo.ts — 미리보기를 거친다). 되돌렸으면 참. */
  undo(move: GitRefMove, kind: UndoKind): Promise<boolean> {
    return undoMove(
      {
        client: this.deps.client,
        projectId: this.deps.projectId,
        ...(this.deps.worktree !== undefined && { worktree: this.deps.worktree }),
        confirm: this.deps.confirm,
        notify: this.deps.notify,
        strings: this.deps.dialogStrings,
      },
      move,
      kind,
    );
  }
}
