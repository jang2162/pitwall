// @vitest-environment happy-dom
// 파일 감시 임대: 창이 `fs.watch` 를 열어 두고, 닫히면(언마운트) 스트림을 취소하고, 활성 워크트리가 바뀌면 닫고 새 워크트리로 다시 연다.

import { createMemoryTransportPair, type FsWatching, RpcClient } from "@pitwall/rpc-client";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { rememberActiveWorktree } from "../open-with/active-worktree";
import { useFileWatch, watchFiles } from "./file-watch";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

/** `fs.watch` 를 열 때마다 새 스트림 id 를 주고 항목 하나를 보내는 가짜 Host. 취소된 스트림 id 를 모은다. */
function fakeHost() {
  const [clientSide, serverSide] = createMemoryTransportPair();
  const opened: { streamId: number; params: unknown }[] = [];
  const cancelled: number[] = [];
  let nextStream = 1;
  const send = (message: object) => serverSide.send(JSON.stringify({ jsonrpc: "2.0", ...message }));
  serverSide.onMessage((text) => {
    const message = JSON.parse(text) as { id?: number; method: string; params: unknown };
    if (message.method === "fs.watch") {
      const streamId = nextStream++;
      opened.push({ streamId, params: message.params });
      send({ id: message.id, result: { streamId } });
      const data: FsWatching = { roots: [{ path: "/repo", mode: "recursive" }] };
      send({ method: "stream.chunk", params: { streamId, data } });
    } else if (message.method === "stream.cancel") {
      cancelled.push((message.params as { streamId: number }).streamId);
    }
  });
  return { client: new RpcClient(clientSide), opened, cancelled };
}

let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  rememberActiveWorktree({ file: "/main/a.txt" });
});

afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
});

it("keeps fs.watch open, reports the watch mode and cancels the stream when disposed", async () => {
  const host = fakeHost();
  const onWatching = vi.fn<(watching: FsWatching) => void>();
  const dispose = watchFiles(host.client, "p1", "/wt", onWatching);
  await vi.waitFor(() => expect(onWatching).toHaveBeenCalledOnce());
  expect(onWatching.mock.calls[0]![0].roots[0]!.mode).toBe("recursive");
  expect(host.opened).toEqual([{ streamId: 1, params: { projectId: "p1", worktree: "/wt" } }]);
  expect(host.cancelled).toEqual([]);

  dispose();
  await vi.waitFor(() => expect(host.cancelled).toEqual([1]));
});

function Watcher({ client, projectId }: { client: RpcClient; projectId: string }) {
  useFileWatch(client, projectId);
  return null;
}

it("re-attaches to the new worktree when the active worktree changes, and lets go on unmount", async () => {
  const host = fakeHost();
  const element = document.createElement("div");
  document.body.append(element);
  root = createRoot(element);
  await act(async () => root!.render(<Watcher client={host.client} projectId="p1" />));
  await vi.waitFor(() => expect(host.opened).toHaveLength(1));
  expect(host.opened[0]!.params).toEqual({ projectId: "p1", worktree: null });

  await act(async () => rememberActiveWorktree({ worktree: "/wt" }));
  await vi.waitFor(() => expect(host.opened).toHaveLength(2));
  expect(host.opened[1]!.params).toEqual({ projectId: "p1", worktree: "/wt" });
  await vi.waitFor(() => expect(host.cancelled).toEqual([1]));

  // 같은 워크트리를 다시 받으면 다시 열지 않는다.
  await act(async () => rememberActiveWorktree({ worktree: "/wt", file: "/wt/a.txt" }));
  expect(host.opened).toHaveLength(2);

  act(() => root!.unmount());
  root = undefined;
  await vi.waitFor(() => expect(host.cancelled).toEqual([1, 2]));
});
