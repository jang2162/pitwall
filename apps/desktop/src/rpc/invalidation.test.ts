import { createMemoryTransportPair, type HostInfo, RpcClient } from "@pitwall/rpc-client";
import { QueryClient, QueryObserver } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";

import { connectEventInvalidation } from "./invalidation";
import { rpcQueryKey, rpcQueryOptions } from "./queries";

const HOST = "host:local";

/** host.info 와 events.subscribe/unsubscribe 만 아는 가짜 Host. `emit` 으로 구독자에게 이벤트를 보낸다. */
function fakeHost() {
  const [clientSide, serverSide] = createMemoryTransportPair();
  const subscriptions = new Map<number, { projectId: string; topics: string[] }>();
  let nextSubscription = 1;
  let version = 0;
  const infoCalls = vi.fn();
  const unsubscribeCalls = vi.fn();
  serverSide.onMessage((text) => {
    const message = JSON.parse(text) as { id?: number; method: string; params: unknown };
    const reply = (result: unknown) =>
      serverSide.send(JSON.stringify({ jsonrpc: "2.0", id: message.id, result }));
    switch (message.method) {
      case "host.info": {
        infoCalls();
        version++;
        reply({
          hostId: "local",
          kind: "local",
          version: `0.0.${version}`,
          os: "test",
          arch: "test",
        } satisfies HostInfo);
        return;
      }
      case "events.subscribe": {
        const id = nextSubscription++;
        subscriptions.set(id, message.params as { projectId: string; topics: string[] });
        reply({ subscriptionId: id });
        return;
      }
      case "events.unsubscribe": {
        unsubscribeCalls();
        subscriptions.delete((message.params as { subscriptionId: number }).subscriptionId);
        reply(null);
      }
    }
  });
  return {
    client: new RpcClient(clientSide),
    infoCalls,
    unsubscribeCalls,
    subscriptions,
    emit(projectId: string, topic: string, payload: unknown = null) {
      for (const [subscriptionId, sub] of subscriptions) {
        if (sub.projectId !== projectId || !sub.topics.includes(topic)) continue;
        serverSide.send(
          JSON.stringify({
            jsonrpc: "2.0",
            method: "events.event",
            params: { subscriptionId, projectId, topic, payload },
          }),
        );
      }
    },
  };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

let queryClient: QueryClient;

afterEach(() => queryClient?.clear());

function setup() {
  const host = fakeHost();
  queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  // 화면에 붙어 있는 쿼리처럼 observer 를 단다. 무효화는 observer 가 있는 쿼리만 다시 부른다.
  const observer = new QueryObserver(
    queryClient,
    rpcQueryOptions(host.client, HOST, "host.info", null),
  );
  const unsubscribe = observer.subscribe(() => {});
  return { host, observer, unsubscribe };
}

describe("rpc queries", () => {
  it("keys a query by host, method and params", async () => {
    const { host, observer, unsubscribe } = setup();
    await vi.waitFor(() => expect(observer.getCurrentResult().data?.version).toBe("0.0.1"));
    expect(queryClient.getQueryData(rpcQueryKey(HOST, "host.info", null))).toMatchObject({
      hostId: "local",
    });
    expect(host.infoCalls).toHaveBeenCalledTimes(1);
    unsubscribe();
  });
});

describe("connectEventInvalidation", () => {
  it("refetches the queries a matching event invalidates", async () => {
    const { host, observer, unsubscribe } = setup();
    await vi.waitFor(() => expect(observer.getCurrentResult().data?.version).toBe("0.0.1"));

    const invalidation = connectEventInvalidation(host.client, queryClient, "p1", [
      { topic: "host.changed", invalidate: () => [rpcQueryKey(HOST, "host.info")] },
    ]);
    await invalidation.ready;
    expect([...host.subscriptions.values()]).toEqual([
      { projectId: "p1", topics: ["host.changed"] },
    ]);

    host.emit("p1", "host.changed");
    await vi.waitFor(() => expect(observer.getCurrentResult().data?.version).toBe("0.0.2"));
    expect(host.infoCalls).toHaveBeenCalledTimes(2);

    invalidation.dispose();
    unsubscribe();
  });

  it("leaves queries alone for topics no rule names", async () => {
    const { host, observer, unsubscribe } = setup();
    await vi.waitFor(() => expect(observer.getCurrentResult().data).toBeDefined());
    const invalidation = connectEventInvalidation(host.client, queryClient, "p1", [
      { topic: "host.changed", invalidate: () => [rpcQueryKey(HOST, "host.info")] },
      { topic: "fs.changed", invalidate: () => [rpcQueryKey(HOST, "fs.tree" as "host.info")] },
    ]);
    await invalidation.ready;

    host.emit("p1", "fs.changed");
    await settle();
    expect(queryClient.getQueryState(rpcQueryKey(HOST, "host.info", null))?.isInvalidated).toBe(
      false,
    );
    expect(host.infoCalls).toHaveBeenCalledTimes(1);
    invalidation.dispose();
    unsubscribe();
  });

  it("lets a rule pick keys from the event payload", async () => {
    const { host, observer, unsubscribe } = setup();
    await vi.waitFor(() => expect(observer.getCurrentResult().data).toBeDefined());
    const invalidate = vi.fn(() => [rpcQueryKey(HOST, "host.info")]);
    const invalidation = connectEventInvalidation(host.client, queryClient, "p1", [
      { topic: "host.changed", invalidate },
    ]);
    await invalidation.ready;

    host.emit("p1", "host.changed", { reason: "upgrade" });
    await vi.waitFor(() => expect(invalidate).toHaveBeenCalledTimes(1));
    expect(invalidate.mock.calls[0]).toEqual([
      expect.objectContaining({ topic: "host.changed", payload: { reason: "upgrade" } }),
    ]);
    invalidation.dispose();
    unsubscribe();
  });

  it("unsubscribes on dispose and ignores events after it", async () => {
    const { host, observer, unsubscribe } = setup();
    await vi.waitFor(() => expect(observer.getCurrentResult().data).toBeDefined());
    const invalidation = connectEventInvalidation(host.client, queryClient, "p1", [
      { topic: "host.changed", invalidate: () => [rpcQueryKey(HOST)] },
    ]);
    await invalidation.ready;
    invalidation.dispose();
    await vi.waitFor(() => expect(host.unsubscribeCalls).toHaveBeenCalledTimes(1));

    host.emit("p1", "host.changed");
    await settle();
    expect(host.infoCalls).toHaveBeenCalledTimes(1);
    unsubscribe();
  });

  it("closes a subscription that opens after dispose", async () => {
    const { host } = setup();
    const invalidation = connectEventInvalidation(host.client, queryClient, "p1", [
      { topic: "host.changed", invalidate: () => [rpcQueryKey(HOST)] },
    ]);
    invalidation.dispose();
    await invalidation.ready;
    await vi.waitFor(() => expect(host.unsubscribeCalls).toHaveBeenCalledTimes(1));
    expect(host.subscriptions.size).toBe(0);
  });
});
