import { expect, it } from "vitest";

import { activeWorktree, rememberActiveWorktree } from "./active-worktree";

it("follows the worktree of the last open request and returns to the main checkout on a plain file", () => {
  expect(activeWorktree()).toBeUndefined();
  rememberActiveWorktree({ worktree: "/w/wt", file: "/w/wt/a.rs" });
  expect(activeWorktree()).toBe("/w/wt");
  // 창을 앞으로 가져오기만 한 요청은 바꾸지 않는다.
  rememberActiveWorktree({});
  expect(activeWorktree()).toBe("/w/wt");
  rememberActiveWorktree({ file: "/w/main/a.rs" });
  expect(activeWorktree()).toBeUndefined();
});
