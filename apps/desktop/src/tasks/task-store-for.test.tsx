// @vitest-environment happy-dom
// 작업 표는 연결마다 하나다: 같은 연결이면 같은 표, 연결이 바뀌면 옛 Host 의 작업이 없는 새 표.

import type { RpcClient } from "@pitwall/rpc-client";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it } from "vitest";

import { type TaskStore, useTaskStoreFor } from "./tasks";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

let root: Root | undefined;
const seen: TaskStore[] = [];

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  seen.length = 0;
});

afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
});

function Probe({ client }: { client: RpcClient | undefined }) {
  seen.push(useTaskStoreFor(client));
  return null;
}

it("keeps the store for the same client and starts an empty one for a new client", async () => {
  const a = {} as RpcClient;
  const b = {} as RpcClient;
  root = createRoot(document.createElement("div"));
  await act(async () => root!.render(<Probe client={a} />));
  const first = seen.at(-1)!;
  first.getState().progressed({ taskId: 1, title: "old host", progress: { done: 0 } }, null);

  await act(async () => root!.render(<Probe client={a} />));
  expect(seen.at(-1)).toBe(first);

  await act(async () => root!.render(<Probe client={b} />));
  const second = seen.at(-1)!;
  expect(second).not.toBe(first);
  expect(second.getState().tasks).toEqual([]);

  await act(async () => root!.render(<Probe client={b} />));
  expect(seen.at(-1)).toBe(second);
});
