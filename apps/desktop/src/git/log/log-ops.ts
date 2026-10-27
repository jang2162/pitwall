// Git 로그의 쓰기(docs/design/13-m2-plan.md M2-14 의 우클릭 · 라벨 끌어 놓기 — 실행은 M2-09 · M2-10 RPC). 무엇을 묻고 답대로 무엇을
// 부르는지를 여기서 정하고, 묻는 화면은 M1 의 확인 부품(file-tree/file-dialogs.tsx — 이름 묻기 · 예/아니오)을 그대로 쓴다.
// 부르는 연결은 프로젝트 창의 연결이다 — Host 는 git 쓰기를 그 프로젝트 창의 연결(`ConnectionScope::Project`)로만 받는다.
//
// - reset: soft 는 잃을 것이 없어 묻지 않는다. hard 는 늘, mixed · keep 은 잃을 것이 있을 때만 `git.resetPreview` 의 잃을 변경을
//   보이고 확인한 뒤 그 지문(`fingerprint`)과 `expected`(미리보기 때의 HEAD)를 실어 부른다(결정 M2-D6 ①). 지문이 `null` 이면 확인할 수
//   없어 하지 않는다. 그사이 바뀌어 conflict(`-32004`)가 오면 새 목록으로 한 번 더 묻는다.
// - ref 를 옮긴 결과(`GitRefMove`)가 오면 [되돌리기] 를 단다: `previous` 로 `git.reset --keep`(`expected` = `current`,
//   `expectedRef` = `refName` — 그사이 다른 것이 HEAD 를 옮겼으면 Host 가 거절한다). keep 이 잃을 것이 있으면 conflict 의 목록으로
//   묻고 그 지문으로 다시 부른다.
// - git 에 리비전으로 넘기는 ref(merge · rebase 의 위 · 비교)는 언제나 전체 이름(`refFullName`)이다 — 짧은 이름이면 같은 이름의
//   태그가 브랜치보다 먼저 풀린다. merge · rebase 는 Host 가 실행 직전에 모호하지 않으면 짧은 이름으로 바꿔 메시지를 터미널과
//   맞춘다(pw-git `history` 의 `unambiguous_short`). rebase 할 브랜치(`branch`)와 체크아웃은 Host 가 브랜치로만 다뤄 짧은 이름이다.
// - cherry-pick 은 고른 커밋을 오래된 것부터, revert 는 새것부터 넘긴다(새것부터 되돌려야 뒤 커밋이 앞 커밋의 변경 위에서 풀린다).
// - 충돌로 멈추면(`outcome: stopped`) 경고 알림만 — 계속 · 중단은 커밋 툴 윈도우(M2-11 · M2-16)의 몫이다. git 이 거절하면 그
//   원문을 오류 알림의 본문에 둔다.

import {
  ERROR_CODE,
  type GitLostChanges,
  type LogRef,
  type GitRefMove,
  type GitStatusEntry,
  type ResetMode,
  RpcCallError,
  type RpcClient,
} from "@pitwall/rpc-client";

import type { ConfirmQuestion, NameQuestion } from "../../file-tree/file-ops";
import type { NotificationInput } from "../../notifications/notifications";
import type { GitLogStrings } from "./strings";

/** 목록에 보이는 잃을 항목 수. 넘는 것은 「그리고 n개 더」. */
const LOST_SHOWN = 30;

export interface LogOpsDeps {
  client: RpcClient;
  projectId: string;
  worktree?: string;
  askName(question: NameQuestion): Promise<string | undefined>;
  confirm(question: ConfirmQuestion): Promise<boolean>;
  notify(input: NotificationInput): void;
  strings: GitLogStrings;
  /** ref 가 움직였다 — [되돌리기] 를 단다. */
  moved(move: GitRefMove): void;
}

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

function entryLine(entry: GitStatusEntry): string {
  return entry.origPath ? `${entry.origPath} → ${entry.path}` : entry.path;
}

/** 잃을 변경의 줄(많으면 줄인다). */
export function lostLines(lost: GitLostChanges, strings: GitLogStrings["dialog"]): string {
  const lines = [...lost.changes.map(entryLine), ...lost.overwritten];
  const shown = lines.slice(0, LOST_SHOWN);
  if (lines.length > LOST_SHOWN) shown.push(strings.moreLost(lines.length - LOST_SHOWN));
  return shown.join("\n");
}

function lostCount(lost: GitLostChanges): number {
  return lost.changes.length + lost.overwritten.length;
}

function errorText(e: unknown): string {
  if (e instanceof RpcCallError) {
    const stderr = (e.data as { stderr?: unknown } | undefined)?.stderr;
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

function conflictOf(e: unknown): ResetConflict | undefined {
  if (!(e instanceof RpcCallError) || e.code !== ERROR_CODE.CONFLICT) return undefined;
  return (e.data ?? {}) as ResetConflict;
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
    if (result.moved) this.deps.moved(result.moved);
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
    const T = this.deps.strings.dialog;
    const name = await this.deps.askName({
      title: T.newBranch,
      hint: T.from(short(hash)),
      initial: "",
      validate: (n) => refNameProblem(n, T),
    });
    if (name === undefined) return;
    await this.run(label, () =>
      this.deps.client.call("git.branch.create", { ...this.base, name, start: hash }),
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

  /** 잃을 것을 보이고 묻는다. 확인할 수 없으면(지문 `null`) 알리고 거짓. */
  private async confirmLost(
    title: string,
    lost: GitLostChanges,
    fingerprint: string | null,
    always: boolean,
    undo: boolean,
  ): Promise<boolean> {
    const T = this.deps.strings.dialog;
    if (fingerprint === null) {
      this.deps.notify({ level: "error", title, body: T.cannotConfirm });
      return false;
    }
    const count = lostCount(lost);
    if (count === 0 && !always) return true;
    return this.deps.confirm({
      title,
      body:
        count === 0
          ? T.resetNothing
          : `${undo ? T.undoLost(count) : T.resetLost(count)}\n${lostLines(lost, T)}`,
      confirm: undo ? T.undoConfirm : T.resetConfirm,
      danger: count > 0,
    });
  }

  /** 이 커밋으로 reset(머리 주석). */
  async reset(hash: string, mode: ResetMode, label: string) {
    if (mode === "soft") {
      const result = await this.run(label, () =>
        this.deps.client.call("git.reset", { ...this.base, commit: hash, mode }),
      );
      if (result?.moved) this.deps.moved(result.moved);
      return;
    }
    const preview = await this.run(label, () =>
      this.deps.client.call("git.resetPreview", { ...this.base, commit: hash, mode }),
    );
    if (!preview) return;
    const title = this.deps.strings.dialog.reset(mode, short(hash));
    let { head, lost, fingerprint } = preview;
    // 한 번 다시 묻는다(그사이 작업 트리가 바뀌었으면). 그 뒤에도 바뀌면 오류로 알린다.
    for (let attempt = 0; attempt < 2; attempt++) {
      if (!(await this.confirmLost(title, lost, fingerprint, mode === "hard", false))) return;
      try {
        const result = await this.deps.client.call("git.reset", {
          ...this.base,
          commit: hash,
          mode,
          expected: head,
          fingerprint,
        });
        if (result.moved) this.deps.moved(result.moved);
        return;
      } catch (e) {
        const conflict = conflictOf(e);
        if (!conflict?.lost || attempt > 0) {
          this.deps.notify({
            level: "error",
            title: this.deps.strings.result.failed(label),
            body: errorText(e),
          });
          return;
        }
        this.deps.notify({ level: "info", title: this.deps.strings.result.changed });
        head = conflict.head ?? head;
        lost = conflict.lost;
        fingerprint = conflict.fingerprint ?? null;
      }
    }
  }

  /** [되돌리기]: 움직이기 전으로 `reset --keep`(머리 주석). 되돌렸으면 참. */
  async undo(move: GitRefMove, label: string): Promise<boolean> {
    if (move.previous === null) return false;
    const previous = move.previous;
    const call = (fingerprint?: string | null) =>
      this.deps.client.call("git.reset", {
        ...this.base,
        commit: previous,
        mode: "keep",
        expected: move.current,
        expectedRef: move.refName,
        fingerprint: fingerprint ?? null,
      });
    try {
      await call();
      return true;
    } catch (e) {
      const conflict = conflictOf(e);
      if (!conflict?.lost) {
        this.deps.notify({
          level: "error",
          title: this.deps.strings.result.failed(label),
          body: errorText(e),
        });
        return false;
      }
      const title = this.deps.strings.dialog.undo(short(previous));
      if (!(await this.confirmLost(title, conflict.lost, conflict.fingerprint ?? null, true, true)))
        return false;
      const done = await this.run(label, () => call(conflict.fingerprint));
      return done !== undefined;
    }
  }
}
