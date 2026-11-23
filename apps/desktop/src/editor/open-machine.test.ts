// @vitest-environment happy-dom
// 열기 요청의 머신(18-m7-plan M7-11 리뷰 B2 · 델타 리뷰 B3 · F4): 접두 없는 경로는 요청이 실은 머신의 것이고, 머신이 없을 때만 활성
// 체크아웃의 머신이다. 입구(`workspace.open`)에서 정한 꼴을 탭(`openTab`)이 다시 바꾸지 않는다. 실제 작업 공간 + dockview 로 본다.

import type { FsStat } from "@pitwall/rpc-client";
import { createDockview } from "dockview-react";
import { afterEach, expect, it } from "vitest";

import { checkoutKey, qualify } from "../checkout/checkout";
import { activeWorktree, activeWorktreeStore } from "../open-with/active-worktree";
import { createDocumentStore } from "./documents";
import { tabParams } from "./editor-tabs";
import { createEditorWorkspace } from "./editor-workspace";

const MINI = "SHA256:mini/fingerprint";

afterEach(() => {
  activeWorktreeStore.setState({ top: undefined });
  document.body.innerHTML = "";
});

function setup() {
  const el = document.createElement("div");
  document.body.append(el);
  const api = createDockview(el, {
    createComponent: () => ({ element: document.createElement("div"), init: () => {} }),
  });
  api.layout(1280, 760);
  const stat = async (path: string): Promise<FsStat> => ({
    path,
    kind: "file",
    target: null,
    size: 1,
    mtimeMs: 1,
    readonly: false,
    sizeClass: "editable",
  });
  const stats: { path: string; worktree: string | undefined }[] = [];
  const workspace = createEditorWorkspace({
    documents: createDocumentStore({
      stat,
      read: async () => {
        throw new Error("not read in this test");
      },
    }),
    stat: async (path, worktree) => {
      stats.push({ path, worktree });
      return stat(path);
    },
    onOpenFailed() {},
    onSaveFailed() {},
    onSaveBlocked() {},
    confirmUnsaved: async () => "cancel",
  });
  workspace.attach(api);
  return { workspace, stats };
}

it("keeps a request that names this machine on this machine while another machine is active", async () => {
  const { workspace, stats } = setup();
  activeWorktreeStore.setState({ top: checkoutKey({ machine: MINI }) });
  const panel = await workspace.open({ path: "/work/a.rs", machine: "local" });
  expect(stats).toEqual([{ path: "/work/a.rs", worktree: undefined }]);
  expect(tabParams(panel!).path).toBe("/work/a.rs");
  expect(tabParams(panel!).worktree).toBeUndefined();
});

it("reads a path without a machine as the active checkout's, worktree included", async () => {
  const { workspace, stats } = setup();
  const remoteWorktree = checkoutKey({ machine: MINI, worktree: "/work/wt" });
  activeWorktreeStore.setState({ top: remoteWorktree });
  // 활성 체크아웃 아래 기능은 그 머신의 경로와 워크트리를 접두 없이 넘긴다(파일 트리의 `{ path, worktree: activeWorktree() }`).
  expect(activeWorktree()).toBe("/work/wt");
  const panel = await workspace.open({ path: "/work/wt/a.rs", worktree: activeWorktree() });
  expect(stats).toEqual([{ path: qualify(MINI, "/work/wt/a.rs"), worktree: remoteWorktree }]);
  expect(tabParams(panel!).path).toBe(qualify(MINI, "/work/wt/a.rs"));
  expect(tabParams(panel!).worktree).toBe(remoteWorktree);
  // 이미 머신이 붙은 경로는 다시 붙지 않는다.
  const again = await workspace.open({
    path: qualify(MINI, "/work/wt/a.rs"),
    worktree: remoteWorktree,
  });
  expect(again).toBe(panel);
});
