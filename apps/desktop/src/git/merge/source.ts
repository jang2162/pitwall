// 충돌 해결 화면이 부르는 길(13-m2-plan M2-19): M2-11 의 `git.conflicts` · `git.conflict.get` · `git.conflict.resolve` 와 M2-06 의
// `git.operation.continue` · `skip` · `abort`. 프로젝트 창의 `GitMerge`(git-merge.tsx)가 그 창의 Host 연결(그 프로젝트의 연결 범위 —
// 쓰기는 그 연결만 통과한다, 03-architecture.md 「쓰기의 연결 범위」)로 하나를 걸고, 머지 문서 · 목록 다이얼로그 · 배너가 읽는다.
// 창마다 웹뷰가 따로라 모듈 상태로 둔다(src/git/documents/source.ts 와 같다). 시험은 가짜를 건다.
//
// 해결은 늘 `git.conflict.get` 이 준 `version` 을 `expectedVersion` 으로 싣는다 — 한쪽 고르기도 그렇다(목록은 `version` 을 주지
// 않으므로 누르기 전에 get 한다, `pickSide`). 그사이 파일이 바뀌었으면 Host 가 `CONFLICT` 로 거절하고 아무것도 쓰지 않는다.

import {
  type ConflictFile,
  type ConflictPick,
  type GitConflictDetail,
  type GitConflictResolveParams,
  type GitConflicts,
  type GitOperationResult,
  type OperationAction,
  ERROR_CODE,
  RpcCallError,
} from "@pitwall/rpc-client";
import { create } from "zustand";

import type { PickOnlyReason } from "./strings";

type Scoped<P> = Omit<P, "projectId" | "worktree">;

export interface MergeSource {
  conflicts(worktree: string | undefined): Promise<GitConflicts>;
  conflict(path: string, worktree: string | undefined): Promise<GitConflictDetail>;
  resolve(
    params: Scoped<GitConflictResolveParams>,
    worktree: string | undefined,
  ): Promise<GitConflicts>;
  operation(action: OperationAction, worktree: string | undefined): Promise<GitOperationResult>;
}

export const mergeSource = create<{ source: MergeSource | undefined }>(() => ({
  source: undefined,
}));

/** `CONFLICT`(그사이 파일이 바뀜). */
export function isConflict(e: unknown): boolean {
  return e instanceof RpcCallError && e.code === ERROR_CODE.CONFLICT;
}

/** 충돌이 아니게 됐다(다른 곳에서 풀렸다 — get 이 not found). */
export function isGone(e: unknown): boolean {
  return e instanceof RpcCallError && e.code === ERROR_CODE.NOT_FOUND;
}

/** 한쪽을 고른다: 먼저 get 으로 지금 `version` 을 받아 그것을 싣는다. */
export async function pickSide(
  source: MergeSource,
  path: string,
  pick: ConflictPick,
  worktree: string | undefined,
): Promise<GitConflicts> {
  const { version } = await source.conflict(path, worktree);
  return source.resolve({ path, side: { pick, expectedVersion: version } }, worktree);
}

/** 3-way 머지 문서로 열 수 없는 까닭(한쪽 고르기만 — 결정 M2-D4, M2-11 의 `large`). 열 수 있으면 `undefined`. */
export function pickOnlyReason(file: ConflictFile): PickOnlyReason | undefined {
  if (file.submodule) return "submodule";
  if (file.symlink) return "symlink";
  if (file.binary) return "binary";
  if (file.large || file.tooLarge) return "large";
  if (!file.ours || !file.theirs) return "deleted";
  return undefined;
}

/** 머지 문서 탭의 상대 경로에서 파일 이름. */
export function fileName(path: string): string {
  return path.split("/").pop() ?? path;
}
