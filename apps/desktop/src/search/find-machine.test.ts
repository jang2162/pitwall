// 델타 리뷰 F3(18-m7-plan M7-11): 찾기 결과는 검색을 돌린 머신의 경로다. 고정한 결과를 활성 체크아웃을 바꾼 뒤 눌러도 **찾을 때의
// 머신**으로 연다 — 누른 때의 활성 머신으로 읽지 않는다.

import type { SearchTextChunk } from "@pitwall/rpc-client";
import { afterEach, expect, it, vi } from "vitest";

import { checkoutKey } from "../checkout/checkout";
import type { EditorWorkspace } from "../editor/editor-workspace";
import { activeWorktreeStore } from "../open-with/active-worktree";
import { openSnapshotRow } from "./find-in-files";
import { DEFAULT_OPTIONS, FindSession, searchParams } from "./find-session";

const MINI = "SHA256:mini/fingerprint";

afterEach(() => activeWorktreeStore.setState({ top: undefined }));

it("opens a pinned result on the machine that ran the search after the active checkout moved", async () => {
  const file = {
    path: "/work/demo/src/lib.rs",
    lines: [
      { line: 3, preview: "foo", previewStart: 0, ranges: [{ start: 0, end: 3 }], more: false },
    ],
  };
  const session = new FindSession(async function* (): AsyncIterable<SearchTextChunk> {
    yield { kind: "files", files: [file] } as SearchTextChunk;
    yield { kind: "done", truncated: false } as SearchTextChunk;
  });
  activeWorktreeStore.setState({ top: checkoutKey({ machine: MINI }) });
  await session.start(
    searchParams(
      { ...DEFAULT_OPTIONS, query: "foo" },
      { projectId: "r1", worktree: undefined, root: "/work/demo" },
    ),
  );
  const snapshot = session.getSnapshot();
  expect(snapshot.machine).toBe(MINI);

  // 활성 체크아웃을 이 머신으로 바꾼 뒤 고정한 결과를 누른다.
  activeWorktreeStore.setState({ top: undefined });
  const navigateTo = vi.fn(async () => true);
  const workspace = { navigateTo } as unknown as EditorWorkspace;
  // 결과를 여는 화면(팝업 · 고정 결과)이 부르는 자리 그대로.
  openSnapshotRow(workspace, snapshot, snapshot.rows[0]!);
  expect(navigateTo).toHaveBeenCalledWith(
    expect.objectContaining({ path: file.path, machine: MINI }),
    { record: true },
  );
});
