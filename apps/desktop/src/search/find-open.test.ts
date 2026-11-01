// @vitest-environment happy-dom
// 전체에서 찾기의 결과 한 줄을 열면 위치로 가기로 연다(find-in-files.tsx 의 `openFindRow`, 15-m4-plan M4-09): 떠나는 자리가 내비게이션
// 기록에 들고(`record`), 줄은 결과의 줄, 열은 첫 일치 + 1, 워크트리는 결과를 찾은 곳이다.

import { expect, it, vi } from "vitest";

import type { EditorWorkspace } from "../editor/editor-workspace";
import { openFindRow } from "./find-in-files";

it("opens a result row through navigateTo with record, at the first match", () => {
  const navigateTo = vi.fn(async () => true);
  const workspace = { navigateTo } as unknown as EditorWorkspace;
  const row = {
    file: { path: "/work/demo/src/lib.rs", lines: [] },
    line: {
      line: 42,
      preview: "let x = foo();",
      previewStart: 0,
      ranges: [{ start: 8, end: 11 }],
      more: false,
    },
  };
  openFindRow(workspace, row, "/work/wt");
  expect(navigateTo).toHaveBeenCalledWith(
    { path: "/work/demo/src/lib.rs", worktree: "/work/wt", line: 42, column: 9 },
    { record: true },
  );
  // 메인 체크아웃이면 워크트리 칸이 없다.
  openFindRow(workspace, row, undefined);
  expect(navigateTo).toHaveBeenLastCalledWith(
    { path: "/work/demo/src/lib.rs", line: 42, column: 9 },
    { record: true },
  );
});
