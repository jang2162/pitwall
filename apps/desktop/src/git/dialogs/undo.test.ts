// [되돌리기] 의 낱개 규칙(dialogs/undo.ts): 지운 브랜치는 그 해시에 다시 만들고, 만든 브랜치는 지우되 병합 안 됐으면(2101) 원문과
// 함께 물은 뒤에만 강제로; 확인할 수 없는 미리보기(지문 `null`)면 확인을 눌러도 reset 하지 않는다; 그사이 HEAD 가 움직여 Host 가
// 새 목록 없이 거절하면 그 거절을 그대로 보이고 다시 부르지 않는다; `git.undo.offer` 의 인자 검사와 단추를 달 수 있는지.

import { ERROR_CODE, type GitRefMove, RpcCallError, type RpcClient } from "@pitwall/rpc-client";
import { expect, it, vi } from "vitest";

import { bundledTranslator } from "../../i18n/testing";
import type { NotificationInput } from "../../notifications/notifications";
import type { GitConfirm } from "./git-dialogs";
import { undoOfferArgs } from "./git-remote-feature";
import { gitDialogStrings } from "./strings";
import { NOT_MERGED, canUndo, offerUndo, undoMove } from "./undo";

const T = gitDialogStrings(bundledTranslator("ko"));

function depsWith(answer: (method: string, params: Record<string, unknown>) => unknown, ok = true) {
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  const confirms: GitConfirm[] = [];
  const notes: NotificationInput[] = [];
  const client = {
    call: vi.fn(async (method: string, params: Record<string, unknown>) => {
      calls.push({ method, params });
      return answer(method, params);
    }),
  } as unknown as RpcClient;
  return {
    deps: {
      client,
      projectId: "p1",
      worktree: "/wt",
      confirm: async (q: GitConfirm) => {
        confirms.push(q);
        return ok;
      },
      notify: (n: NotificationInput) => notes.push(n),
      strings: T,
    },
    calls,
    confirms,
    notes,
  };
}

const moved: GitRefMove = {
  refName: "refs/heads/main",
  previous: "p".repeat(40),
  current: "c".repeat(40),
};

it("restores a deleted branch at the commit it pointed at, without checking it out", async () => {
  const { deps, calls, notes } = depsWith(() => ({}));
  const done = await undoMove(
    deps,
    { refName: "refs/heads/feat/x", previous: "f".repeat(40), current: null },
    { kind: "branchDeleted" },
  );
  expect(done).toBe(true);
  expect(calls).toEqual([
    {
      method: "git.branch.create",
      params: {
        projectId: "p1",
        worktree: "/wt",
        name: "feat/x",
        start: "f".repeat(40),
        checkout: false,
      },
    },
  ]);
  expect(notes[0]!.title).toBe(T.undo.restored("feat/x"));
});

it("removes a created branch, forcing only after showing git's refusal and asking", async () => {
  const { deps, calls, confirms } = depsWith((_m, p) => {
    if (!p.force)
      throw new RpcCallError({
        code: NOT_MERGED,
        message: "not merged",
        data: { stderr: "error: the branch 'x' is not fully merged", commit: "c" } as never,
      });
    return {};
  });
  const done = await undoMove(
    deps,
    { refName: "refs/heads/x", previous: null, current: "c".repeat(40) },
    { kind: "branchCreated" },
  );
  expect(done).toBe(true);
  expect(confirms[0]!.danger).toBe(true);
  expect(confirms[0]!.detail).toContain("not fully merged");
  expect(calls.map((c) => c.params.force)).toEqual([false, true]);
});

it("does not reset when the undo preview cannot be fingerprinted, whatever the dialog answers", async () => {
  const { deps, calls, confirms } = depsWith(() => ({
    workTree: "/r",
    head: moved.current,
    target: moved.previous,
    lost: { changes: [], overwritten: ["dir/"] },
    fingerprint: null,
  }));
  expect(await undoMove(deps, moved, { kind: "head", mode: "keep" })).toBe(false);
  expect(confirms[0]!.lost!.fingerprint).toBeNull();
  expect(calls.map((c) => c.method)).toEqual(["git.resetPreview"]);
});

it("shows the Host's refusal as it is when HEAD moved since, and does not try again", async () => {
  const { deps, calls, notes } = depsWith((method) => {
    if (method === "git.resetPreview")
      return {
        workTree: "/r",
        head: "x".repeat(40),
        target: moved.previous,
        lost: { changes: [], overwritten: [] },
        fingerprint: "f",
      };
    throw new RpcCallError({
      code: ERROR_CODE.CONFLICT,
      message: "HEAD is no longer at cccccccc",
      data: { head: "x".repeat(40), refName: "refs/heads/main" } as never,
    });
  });
  expect(await undoMove(deps, moved, { kind: "head", mode: "keep" })).toBe(false);
  expect(calls.map((c) => c.method)).toEqual(["git.resetPreview", "git.reset"]);
  expect(notes).toEqual([
    { level: "error", title: T.undo.failed, body: "HEAD is no longer at cccccccc (-32004)" },
  ]);
});

it("checks git.undo.offer's arguments and puts the button only where an undo is possible", () => {
  expect(undoOfferArgs({ move: moved })).toEqual({ move: moved, mode: "keep" });
  expect(undoOfferArgs({ move: moved, mode: "soft" })).toEqual({ move: moved, mode: "soft" });
  for (const bad of [undefined, {}, { move: { refName: 1 } }, { move: moved, mode: "hard" }])
    expect(undoOfferArgs(bad), JSON.stringify(bad)).toBeUndefined();

  expect(canUndo({ ...moved, previous: null }, { kind: "head", mode: "keep" })).toBe(false);
  const { deps, notes } = depsWith(() => ({}));
  offerUndo(deps, moved, { kind: "head", mode: "soft" });
  offerUndo(deps, { ...moved, previous: null }, { kind: "head", mode: "keep" });
  expect(notes[0]!.title).toBe(T.undo.moved("main", "pppppppp", "cccccccc"));
  expect(notes[0]!.action?.label).toBe(T.undo.button);
  expect(notes[1]!.action).toBeUndefined();
});
