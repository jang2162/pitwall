import type { EventMessage, SubscribeParams } from "@pitwall/rpc-client";
import { expect, it } from "vitest";

import { projectSubscriptions, type SubscribeClient } from "./project-subscriptions";

function fakeClient() {
  const subscribed: SubscribeParams[] = [];
  const unsubscribed: number[] = [];
  const listeners: ((e: EventMessage) => void)[] = [];
  const client: SubscribeClient = {
    async subscribe(params, listener) {
      subscribed.push(params);
      listeners.push(listener);
      const id = subscribed.length;
      return { id, unsubscribe: async () => void unsubscribed.push(id) };
    },
  };
  const emit = (topic: string) =>
    listeners.forEach((l) => l({ subscriptionId: 0, projectId: "p1", topic, payload: { topic } }));
  return { client, subscribed, unsubscribed, emit };
}

const settle = () => new Promise((r) => setTimeout(r, 0));

it("waits for the window's Host connection instead of throwing, then subscribes (N-4)", async () => {
  const subs = projectSubscriptions("p1");
  const got: unknown[] = [];
  const off = subs.subscribe(["lsp.diagnostics"], (e) => got.push(e.payload));
  const dropped = subs.subscribe(["lsp.other"], () => {});
  dropped();
  const host = fakeClient();
  subs.connect(undefined);
  subs.connect(host.client);
  await settle();
  // 붙기 전에 해제한 것은 구독하지 않는다.
  expect(host.subscribed).toEqual([{ projectId: "p1", topics: ["lsp.diagnostics"] }]);
  host.emit("lsp.diagnostics");
  expect(got).toEqual([{ topic: "lsp.diagnostics" }]);
  off();
  host.emit("lsp.diagnostics");
  expect(got).toHaveLength(1);
  expect(host.unsubscribed).toEqual([1]);
  // 연결이 있으면 곧바로 붙는다.
  subs.subscribe(["lsp.status"], () => {});
  await settle();
  expect(host.subscribed).toHaveLength(2);
});
