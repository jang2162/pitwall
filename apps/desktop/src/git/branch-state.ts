// 창 하나의 브랜치 상태(13-m2-plan.md M2-13): 상태바의 브랜치 항목 · 브랜치 팝업 · Search Everywhere 의 브랜치 탭이 함께 읽는
// `git.refs`(HEAD · 로컬/원격 브랜치 · 태그 · upstream 과 ahead/behind)와 `git.operation`(진행 중 작업).
//
// - 감시를 따로 열지 않는다. 파일 트리 · 탭 색이 연 `git.watch`(git-status.ts 의 `GitStatusWatch`)가 내는 `git.refsChanged` 를 구독해
//   다시 읽는다 — Host 의 감시는 (프로젝트, 활성 워크트리)마다 하나다(crates/pw-git/src/watch/mod.rs).
// - 구독을 건 뒤 한 번 읽고, 창의 status(`windowGitStatus()`)의 작업 트리가 바뀔 때(감시가 처음 열렸다 · 저장소가 생겼다 · 사라졌다)도
//   다시 읽는다 — 감시가 열리기 전에 바뀐 것을 놓치지 않게.
// - 읽기는 한 번에 하나다. 읽는 동안 온 이벤트는 하나로 모아 끝난 뒤 한 번 더 읽는다.
// - 활성 워크트리가 바뀌면 프로젝트 창의 `GitBranchWatch` 가 닫고 비운 뒤 새 워크트리로 다시 연다.

import type { GitRefs, OperationInfo, RpcClient } from "@pitwall/rpc-client";
import { useEffect, useSyncExternalStore } from "react";

import { useActiveWorktree } from "../open-with/active-worktree";
import { useHost } from "../rpc/host";
import { type GitStatusStore, windowGitStatus } from "./git-status";

export interface BranchSnapshot {
  /** 마지막으로 읽은 ref. 아직 읽지 않았으면 없다. `workTree: null` 이면 저장소가 없다. */
  readonly refs?: GitRefs;
  readonly operation: OperationInfo | null;
  /** 마지막 읽기가 실패했으면 그 까닭. */
  readonly error?: string;
}

const EMPTY: BranchSnapshot = Object.freeze({ operation: null });

export class BranchStore {
  private snapshot: BranchSnapshot = EMPTY;
  private readonly listeners = new Set<() => void>();
  /** 읽은 횟수(시험이 「바뀐 것만 한 번 읽었다」 를 본다). */
  reads = 0;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  get = (): BranchSnapshot => this.snapshot;

  set(next: BranchSnapshot) {
    this.snapshot = next;
    for (const listener of this.listeners) listener();
  }

  clear() {
    if (this.snapshot !== EMPTY) this.set(EMPTY);
  }
}

let windowStore: BranchStore | undefined;

/** 이 창의 브랜치 상태. */
export function windowBranch(): BranchStore {
  windowStore ??= new BranchStore();
  return windowStore;
}

/** 시험용: 창의 브랜치 상태를 버린다. */
export function resetWindowBranch() {
  windowStore = undefined;
}

export function useBranch(store: BranchStore = windowBranch()): BranchSnapshot {
  return useSyncExternalStore(store.subscribe, store.get);
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * `projectId` · `worktree` 의 ref 를 `store` 에 채운다. `status` 는 같은 창의 git status(작업 트리가 바뀌면 다시 읽는다). 돌려준
 * 함수가 구독을 닫고 `store` 를 비운다.
 */
export function watchBranch(
  client: RpcClient,
  projectId: string,
  worktree: string | undefined,
  store: BranchStore,
  status: GitStatusStore = windowGitStatus(),
): () => void {
  const abort = new AbortController();
  const stopped = () => abort.signal.aborted;
  let reading = false;
  let again = false;
  let unsubscribe: (() => Promise<void>) | undefined;

  const read = async () => {
    if (stopped()) return;
    if (reading) {
      again = true;
      return;
    }
    reading = true;
    again = false;
    const params = { projectId, worktree: worktree ?? null };
    try {
      store.reads++;
      const [refs, operation] = await Promise.all([
        client.call("git.refs", params, { signal: abort.signal }),
        client.call("git.operation", params, { signal: abort.signal }),
      ]);
      if (!stopped()) store.set({ refs, operation: operation.operation });
    } catch (e) {
      if (!stopped()) {
        console.warn("[git] git.refs failed", errorText(e));
        store.set({ ...store.get(), error: errorText(e) });
      }
    } finally {
      reading = false;
    }
    if (again) void read();
  };

  let tree = status.workTree();
  const offStatus = status.subscribe(() => {
    const next = status.workTree();
    if (next === tree) return;
    tree = next;
    void read();
  });

  void (async () => {
    try {
      const subscription = await client.subscribe(
        { projectId, topics: ["git.refsChanged"] },
        (event) => {
          const change = event.payload as unknown as { worktree: string | null };
          if ((change.worktree ?? undefined) !== worktree) return;
          void read();
        },
      );
      unsubscribe = () => subscription.unsubscribe();
      if (stopped()) {
        void unsubscribe().catch(() => {});
        return;
      }
      void read();
    } catch (e) {
      if (!stopped()) console.warn("[git] cannot follow git.refsChanged", errorText(e));
    }
  })();

  return () => {
    abort.abort();
    offStatus();
    void unsubscribe?.().catch(() => {});
    store.clear();
  };
}

/** 이 창의 활성 워크트리의 ref 를 `windowBranch()` 에 채운다. 프로젝트 창에 하나 둔다. */
export function GitBranchWatch({ projectId }: { projectId: string }) {
  const { client } = useHost();
  const worktree = useActiveWorktree();
  useEffect(() => {
    if (!client) return;
    return watchBranch(client, projectId, worktree, windowBranch());
  }, [client, projectId, worktree]);
  return null;
}
