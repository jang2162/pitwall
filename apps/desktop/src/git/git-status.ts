// 창 하나의 git 작업 트리 상태(docs/design/13-m2-plan.md M2-12): `git.watch` 를 열어 두고 `git.statusChanged` 가 오면 `git.status` 를
// 바뀐 경로만 다시 읽어, 파일 트리 · 에디터 탭이 칠할 상태(글자 + 색 토큰)를 경로마다 내준다. 창마다 웹뷰가 따로라 모듈 상태로 둔다.
// 커밋 툴 윈도우(M2-16) · 상태바(M2-13)도 같은 저장소를 읽는다 — `windowGitStatus()` 의 `entries()` · `workTree()` · `unsupported()`.
//
// - 수명: `watchGitStatus` 가 연 `git.watch` 스트림과 `git.statusChanged` 구독은 돌려준 함수로 닫는다. 프로젝트 창의 `GitStatusWatch`
//   가 활성 워크트리마다 열고, 활성 워크트리가 바뀌면 닫고 상태를 비운 뒤 새 워크트리로 다시 연다(`stream.cancel`). 창이 닫히면
//   Host 연결이 함께 닫혀 Host 가 그 연결의 스트림을 놓는다(`fs.watch` 와 같다). Host 는 마지막 스트림이 닫히면 감시를 놓는다
//   (crates/pw-git/src/watch/mod.rs).
// - 순서: 구독을 먼저 걸고 스트림을 연다. 스트림의 첫 항목(감시를 다 건 뒤) 다음에 전체를 한 번 읽는다 — 그 뒤의 바뀜은 이벤트로 온다.
// - 읽기는 한 번에 하나다. 읽는 동안 온 이벤트의 경로는 모았다가 다음 읽기로 보낸다(`paths: null` 이 하나라도 오면 전체).
// - 경로: `git.status` 의 경로는 작업 트리 기준 상대 경로(`/`)이고, 여기서는 `workTree` 를 붙인 절대 경로로 찾는다 — 트리의 논리
//   경로 · 탭의 경로가 작업 트리 아래면 맞는다. 다른 워크트리를 연 탭 · 루트 밖 심링크로 보이는 경로는 칠하지 않는다.
// - 저장소가 없거나(`workTree: null`) gix 로 읽지 못하는 형식(`unsupported`, reftable)이면 아무것도 칠하지 않는다. 「지원하지 않는
//   저장소」 를 알리는 문장은 상태바의 브랜치 항목(M2-13)이 단다.
// - 저장소가 없으면 Host 는 나중의 `git init` 을 따라가지 않는다(crates/pw-git/src/watch/mod.rs). 그동안 같은 워크트리의
//   `fs.changed` 가 오거나 `RETRY_MS` 마다(창이 보일 때만) `git.watch` 를 닫고 다시 연다 — 저장소가 보이면 멈춘다. 주기 재시도가
//   있는 것은 `fs.changed` 가 `.git/` 을 걸러 `git init` 만으로는 아무것도 오지 않기 때문이다. 다시 열 때 Host 는 저장소를 찾기만
//   한다(stat 몇 번).
// - 다른 행(M2-13 · M2-16)은 `windowGitStatus()` 를 읽기만 한다. 같은 저장소로 `watchGitStatus` 를 또 부르면 서로 비운다.
//
// 글자를 고르는 우선순위 · 색 · 폴더 색 · 툴팁의 규칙은 docs/design/10-ui.md 「마우스 · 트랙패드」 의 「Git」 절이 정본이다
// (`entryLetter` · `folderDecoration` · git-status-mark.tsx 가 그것을 옮긴다).

import type {
  GitChange,
  GitConflict,
  GitStatus,
  GitStatusEntry,
  GitUnsupported,
  GitWatching,
  RpcClient,
  Subscription,
} from "@pitwall/rpc-client";
import { useCallback, useEffect, useSyncExternalStore } from "react";

import { type FsChanged, separatorOf } from "../file-tree/tree-model";
import { useActiveWorktree } from "../open-with/active-worktree";
import { useHost } from "../rpc/host";

/** 상태 색 토큰의 마지막 마디(`git.<tone>`). 클래스는 `text-git-<tone>`(styles.css). */
export type GitTone =
  | "added"
  | "modified"
  | "deleted"
  | "renamed"
  | "untracked"
  | "ignored"
  | "conflicted";

/** porcelain 의 글자. `?` 는 untracked, `U` 는 충돌. */
export type GitLetter = "M" | "A" | "D" | "R" | "C" | "T" | "?" | "U";

/** 한 경로를 칠할 것. 같은 내용이면 같은 객체다(행이 바뀐 것만 다시 그리게). */
export interface GitDecoration {
  tone: GitTone;
  /** 폴더의 「아래에 바뀐 것이 있다」 색이면 없다. */
  letter?: GitLetter;
  /** 그 경로 자신의 항목. 폴더 색이면 없다. 툴팁이 두 쪽(스테이징됨 · 작업 트리)을 적는다. */
  staged?: GitChange;
  unstaged?: GitChange;
  conflict?: GitConflict;
}

/** `git.statusChanged` 의 내용(crates/pw-git/src/watch/mod.rs 의 `GitStatusChanged`). 이벤트 타입이라 RPC 생성물에 없다. */
export interface GitStatusChanged {
  worktree: string | null;
  paths: string[] | null;
}

const LETTER: Record<GitChange, GitLetter> = {
  added: "A",
  modified: "M",
  deleted: "D",
  renamed: "R",
  copied: "C",
  typeChanged: "T",
  untracked: "?",
};

const TONE: Record<GitLetter, GitTone> = {
  A: "added",
  C: "added",
  M: "modified",
  T: "modified",
  D: "deleted",
  R: "renamed",
  "?": "untracked",
  U: "conflicted",
};

/** 항목 하나의 글자(머리 주석의 우선순위). */
export function entryLetter(
  entry: Pick<GitStatusEntry, "staged" | "unstaged" | "conflict">,
): GitLetter {
  const { staged, unstaged } = entry;
  if (entry.conflict) return "U";
  if (unstaged === "untracked") return "?";
  if (unstaged === "deleted") return "D";
  if (staged === "added" || staged === "renamed" || staged === "copied") return LETTER[staged];
  if (unstaged) return LETTER[unstaged];
  if (staged) return LETTER[staged];
  // 항목은 어느 쪽이든 바뀐 것만 온다. 둘 다 없으면 바뀐 것으로 본다.
  return "M";
}

const interned = new Map<string, GitDecoration>();

function intern(decoration: GitDecoration): GitDecoration {
  const key = [
    decoration.tone,
    decoration.letter ?? "",
    decoration.staged ?? "",
    decoration.unstaged ?? "",
    decoration.conflict ?? "",
  ].join("|");
  let found = interned.get(key);
  if (!found) interned.set(key, (found = Object.freeze(decoration)));
  return found;
}

/** 항목 하나를 칠할 것. */
export function entryDecoration(entry: GitStatusEntry): GitDecoration {
  const letter = entryLetter(entry);
  return intern({
    tone: TONE[letter],
    letter,
    ...(entry.staged && { staged: entry.staged }),
    ...(entry.unstaged && { unstaged: entry.unstaged }),
    ...(entry.conflict && { conflict: entry.conflict }),
  });
}

/** 폴더 아래 항목 수(폴더 색의 근거). */
interface Below {
  total: number;
  conflicted: number;
  untracked: number;
  added: number;
}

function folderDecoration(below: Below): GitDecoration {
  const tone: GitTone =
    below.conflicted > 0
      ? "conflicted"
      : below.untracked === below.total
        ? "untracked"
        : below.added === below.total
          ? "added"
          : "modified";
  return intern({ tone });
}

/** 상대 경로의 조상 폴더(가까운 것부터, 빈 경로 = 작업 트리 꼭대기는 뺀다). */
function ancestors(rel: string): string[] {
  const out: string[] = [];
  for (let at = rel.lastIndexOf("/"); at > 0; at = rel.lastIndexOf("/", at - 1))
    out.push(rel.slice(0, at));
  return out;
}

/** `rel` 이 `paths` 의 어느 경로이거나 그 아래다. */
function withinAny(rel: string, paths: ReadonlySet<string>): boolean {
  if (paths.has(rel)) return true;
  return ancestors(rel).some((dir) => paths.has(dir));
}

export interface GitStatusSnapshot {
  /** 작업 트리 꼭대기(절대 경로). 저장소가 없거나 아직 읽지 않았으면 `null`. */
  workTree: string | null;
  unsupported: GitUnsupported | null;
}

/**
 * 창 하나의 status. 항목을 상대 경로로 들고, 폴더 색을 위해 폴더마다 아래 항목 수를 센다 — 바뀐 경로만 고칠 때 그 조상만 다시 센다.
 */
export class GitStatusStore {
  private tree: string | null = null;
  private sep = "/";
  private reason: GitUnsupported | null = null;
  /** 상대 경로(끝의 `/` 를 뗀 것) → 항목. */
  private readonly items = new Map<string, GitStatusEntry>();
  /** 상대 폴더 경로 → 아래 항목 수. */
  private readonly below = new Map<string, Below>();
  private version = 0;
  private readonly listeners = new Set<() => void>();

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** 바뀔 때마다 오른다. */
  getVersion(): number {
    return this.version;
  }

  private changed() {
    this.version++;
    for (const listener of this.listeners) listener();
  }

  workTree(): string | null {
    return this.tree;
  }

  unsupported(): GitUnsupported | null {
    return this.reason;
  }

  /** 지금 항목(경로순은 아니다). */
  entries(): GitStatusEntry[] {
    return [...this.items.values()];
  }

  /** 비운다(감시를 닫았다). */
  clear() {
    if (this.tree === null && this.reason === null && this.items.size === 0) return;
    this.tree = null;
    this.reason = null;
    this.items.clear();
    this.below.clear();
    this.changed();
  }

  /**
   * `git.status` 의 응답을 넣는다. `paths` 로 불렀으면 그 경로(폴더면 그 아래) 안의 옛 항목을 응답으로 바꾸고, 없으면 통째로
   * 바꾼다. 작업 트리가 바뀌었으면(저장소가 생겼거나 사라졌다) 통째로 바꾼다.
   */
  apply(status: GitStatus, paths?: readonly string[] | null) {
    const whole = !paths || status.workTree !== this.tree;
    this.tree = status.workTree;
    this.sep = status.workTree === null ? "/" : separatorOf(status.workTree);
    this.reason = status.unsupported;
    if (whole || status.unsupported !== null) {
      this.items.clear();
      this.below.clear();
    } else {
      const scope = new Set(paths.map((p) => p.replace(/\/+$/, "")));
      for (const [rel, entry] of this.items)
        if (withinAny(rel, scope)) {
          this.items.delete(rel);
          this.count(rel, entry, -1);
        }
    }
    if (status.workTree !== null && status.unsupported === null)
      for (const entry of status.entries) {
        const rel = entry.path.replace(/\/+$/, "");
        const old = this.items.get(rel);
        if (old) this.count(rel, old, -1);
        this.items.set(rel, entry);
        this.count(rel, entry, 1);
      }
    this.changed();
  }

  private count(rel: string, entry: GitStatusEntry, delta: 1 | -1) {
    const tone = entryDecoration(entry).tone;
    for (const dir of ancestors(rel)) {
      const below = this.below.get(dir) ?? { total: 0, conflicted: 0, untracked: 0, added: 0 };
      below.total += delta;
      if (tone === "conflicted") below.conflicted += delta;
      if (tone === "untracked") below.untracked += delta;
      if (tone === "added") below.added += delta;
      if (below.total === 0) this.below.delete(dir);
      else this.below.set(dir, below);
    }
  }

  /** 절대 경로 → 작업 트리 기준 상대 경로(`/`). 작업 트리 밖이면 `undefined`, 꼭대기면 빈 문자열. */
  relative(path: string): string | undefined {
    const top = this.tree;
    if (top === null) return undefined;
    const base = top.endsWith(this.sep) ? top.slice(0, -1) : top;
    if (path === base) return "";
    if (!path.startsWith(base + this.sep)) return undefined;
    const rel = path.slice(base.length + 1);
    return this.sep === "/" ? rel : rel.split(this.sep).join("/");
  }

  /** 절대 경로(파일 · 폴더)를 칠할 것. 바뀐 것이 없으면 `undefined`. */
  decoration(path: string): GitDecoration | undefined {
    const rel = this.relative(path);
    if (rel === undefined || rel === "") return undefined;
    const entry = this.items.get(rel);
    if (entry) return entryDecoration(entry);
    const below = this.below.get(rel);
    return below ? folderDecoration(below) : undefined;
  }
}

let windowStore: GitStatusStore | undefined;

/** 이 창의 status. */
export function windowGitStatus(): GitStatusStore {
  windowStore ??= new GitStatusStore();
  return windowStore;
}

/** 시험용: 창의 status 를 버린다. */
export function resetWindowGitStatus() {
  windowStore = undefined;
}

/** `path` 를 칠할 것. 그 값이 바뀔 때만 다시 그린다. */
export function useGitDecoration(
  path: string,
  store: GitStatusStore = windowGitStatus(),
): GitDecoration | undefined {
  return useSyncExternalStore(
    useCallback((listener) => store.subscribe(listener), [store]),
    () => store.decoration(path),
  );
}

/** 저장소가 없는 동안 `git.watch` 를 다시 여는 주기. */
export const RETRY_MS = 5000;

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * `projectId` · `worktree`(활성 워크트리의 꼭대기, 메인 체크아웃이면 없음)의 저장소를 감시해 `store` 를 채운다. 돌려준 함수가
 * 스트림 · 구독 · 재시도 타이머를 닫고 `store` 를 비운다. 스트림이 실패하면 경고만 남긴다 — 그 창은 칠하지 않을 뿐이다.
 */
export function watchGitStatus(
  client: RpcClient,
  projectId: string,
  worktree: string | undefined,
  store: GitStatusStore,
  onWatching?: (watching: GitWatching) => void,
): () => void {
  const abort = new AbortController();
  const stopped = () => abort.signal.aborted;
  let unsubscribe: (() => Promise<void>) | undefined;
  /** 지금 연 `git.watch` 를 닫는다(다시 열 때 · 멈출 때). */
  let watch: AbortController | undefined;
  /** 저장소가 없는 동안의 재시도 타이머와 `fs.changed` 구독. */
  let retry: ReturnType<typeof setInterval> | undefined;
  let following: Promise<Subscription | undefined> | undefined;
  /** 다음 읽기에 보낼 경로. `null` 이면 전체, `undefined` 면 읽을 것이 없다. */
  let pending: Set<string> | null | undefined;
  let reading = false;
  /** 첫 전체 읽기 전에는 이벤트의 경로를 모으기만 한다(첫 읽기가 덮는다). */
  let ready = false;

  const request = (paths: readonly string[] | null) => {
    if (paths === null || pending === null) pending = null;
    else {
      pending ??= new Set();
      for (const p of paths) pending.add(p);
    }
    if (ready) void read();
  };

  const read = async () => {
    if (reading || pending === undefined || stopped()) return;
    reading = true;
    const paths = pending === null ? null : [...pending];
    pending = undefined;
    try {
      const status = await client.call(
        "git.status",
        { projectId, worktree: worktree ?? null, paths },
        { signal: abort.signal },
      );
      if (!stopped()) store.apply(status, paths);
    } catch (e) {
      if (!stopped()) console.warn("[git] git.status failed", errorText(e));
    } finally {
      reading = false;
    }
    if (!stopped()) void read();
  };

  const stopRetry = () => {
    clearInterval(retry);
    retry = undefined;
    const subscription = following;
    following = undefined;
    void subscription?.then((s) => s?.unsubscribe()).catch(() => {});
  };

  /** 저장소가 없다고 들었다. 재시도를 건다. */
  const missing = () => {
    store.clear();
    retry ??= setInterval(() => {
      if (typeof document === "undefined" || document.visibilityState !== "hidden") open();
    }, RETRY_MS);
    following ??= client
      .subscribe({ projectId, topics: ["fs.changed"] }, (event) => {
        // 저장소가 없는 동안 작업 트리가 바뀌었다 — `git init` 뒤에 파일을 만들었을 수 있다.
        const change = event.payload as unknown as FsChanged;
        if (retry !== undefined && (change.worktree ?? undefined) === worktree) open();
      })
      .catch((e: unknown) => {
        if (!stopped()) console.warn("[git] cannot follow fs.changed", errorText(e));
        return undefined;
      });
  };

  const open = () => {
    watch?.abort();
    const current = (watch = new AbortController());
    void (async () => {
      try {
        for await (const watching of client.stream(
          "git.watch",
          { projectId, worktree: worktree ?? null },
          { signal: current.signal },
        )) {
          if (current.signal.aborted) break;
          onWatching?.(watching);
          if (watching.workTree === null) {
            missing();
            continue;
          }
          stopRetry();
          ready = true;
          request(null);
        }
      } catch (e) {
        if (!current.signal.aborted) console.warn("[git] the git watch stopped", errorText(e));
      }
    })();
  };

  void (async () => {
    try {
      const subscription = await client.subscribe(
        { projectId, topics: ["git.statusChanged"] },
        (event) => {
          if (stopped()) return;
          const change = event.payload as unknown as GitStatusChanged;
          if ((change.worktree ?? undefined) !== worktree) return;
          request(change.paths);
        },
      );
      unsubscribe = () => subscription.unsubscribe();
      if (stopped()) {
        void unsubscribe().catch(() => {});
        return;
      }
      open();
    } catch (e) {
      if (!stopped()) console.warn("[git] the git watch stopped", errorText(e));
    }
  })();

  return () => {
    abort.abort();
    watch?.abort();
    stopRetry();
    void unsubscribe?.().catch(() => {});
    store.clear();
  };
}

/** 이 창의 활성 워크트리 저장소를 감시해 `windowGitStatus()` 를 채운다. 프로젝트 창에 하나 둔다. */
export function GitStatusWatch({ projectId }: { projectId: string }) {
  const { client } = useHost();
  const worktree = useActiveWorktree();
  useEffect(() => {
    if (!client) return;
    return watchGitStatus(client, projectId, worktree, windowGitStatus());
  }, [client, projectId, worktree]);
  return null;
}
