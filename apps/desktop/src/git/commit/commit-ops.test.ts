// 커밋 툴 윈도우의 쓰기가 고르는 워크트리(M2-16 리뷰 F4): 미리보기 diff 의 쓰기는 그 탭의 워크트리로 간다 — 탭이 메인 체크아웃
// (`undefined`)이면 활성 워크트리가 연결 워크트리여도 `worktree: null` 이다. 툴 윈도우의 쓰기는 누른 때의 활성 워크트리다.

import type { RpcClient } from "@pitwall/rpc-client";
import { expect, it } from "vitest";

import { bundledTranslator } from "../../i18n/testing";
import { createCommitOps } from "./commit-ops";
import { gitCommitStrings } from "./strings";

it("writes from a preview tab to that tab's worktree, null for the main checkout", async () => {
  const sent: { method: string; params: Record<string, unknown> }[] = [];
  const client = {
    call: async (method: string, params: Record<string, unknown>) => {
      sent.push({ method, params });
      return { workTree: "/repo", untracked: [] };
    },
  } as unknown as RpcClient;
  const strings = gitCommitStrings(bundledTranslator("ko"));
  const ops = createCommitOps({
    client: () => client,
    projectId: "p1",
    worktree: () => "/repo/.worktrees/feat",
    confirm: async () => true,
    askName: async () => undefined,
    notify: () => {},
    strings: () => strings,
  });
  const selection = {
    hunk: {
      oldStart: 1,
      oldLines: 0,
      newStart: 1,
      newLines: 1,
      header: "",
      lines: [{ kind: "added" as const, text: "x", oldLine: null, newLine: 1, noNewline: false }],
    },
    lines: [0],
  };
  await ops.stageLines("stage", "a.txt", undefined, selection, undefined);
  await ops.discardLines("a.txt", selection, undefined);
  await ops.stageLines("unstage", "a.txt", undefined, selection, "/repo/.worktrees/other");
  await ops.toggle([
    {
      key: "changes:a.txt",
      group: "changes",
      path: "a.txt",
      origPath: null,
      letter: "M",
      tone: "modified",
      partial: false,
      conflict: null,
      entry: { path: "a.txt", origPath: null, staged: null, unstaged: "modified", conflict: null },
    },
  ]);
  expect(sent.map((c) => [c.method, c.params.worktree])).toEqual([
    ["git.stageLines", null],
    ["git.discardLines", null],
    ["git.unstageLines", "/repo/.worktrees/other"],
    ["git.stage", "/repo/.worktrees/feat"],
  ]);
});
