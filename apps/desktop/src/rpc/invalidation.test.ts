import { createMemoryTransportPair, type HostInfo, RpcClient } from "@pitwall/rpc-client";
import { QueryClient, QueryObserver } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";

import { connectEventInvalidation, type InvalidationRule } from "./invalidation";
import { rpcQueryKey, rpcQueryOptions } from "./queries";

const HOST = "host:local";

interface FakeHostOptions {
  /** host.info 에 바로 답하지 않고 붙잡아 둔다. */
  holdInfo?: boolean;
  /** events.subscribe 에 답한 직후, 같은 흐름에서 이 토픽의 이벤트를 하나 보낸다. */
  emitRightAfterSubscribe?: string;
}

/**
 * host.info · events.subscribe/unsubscribe · request.cancel 만 아는 가짜 Host. `emit` 으로 구독자에게 이벤트를 보낸다.
 * memory transport 는 프레임마다 마이크로태스크 하나로 건네므로, 연달아 보낸 프레임은 그 순서대로 따로 도착한다.
 */
function fakeHost({ holdInfo = false, emitRightAfterSubscribe }: FakeHostOptions = {}) {
  const [clientSide, serverSide] = createMemoryTransportPair();
  const subscriptions = new Map<number, { projectId?: string; topics: string[] }>();
  let nextSubscription = 1;
  let version = 0;
  const infoCalls = vi.fn<(id: number) => void>();
  const unsubscribeCalls = vi.fn();
  const cancels: number[] = [];
  const send = (message: object) => serverSide.send(JSON.stringify({ jsonrpc: "2.0", ...message }));
  const event = (
    subscriptionId: number,
    projectId: string | null,
    topic: string,
    payload: unknown,
  ) => send({ method: "events.event", params: { subscriptionId, projectId, topic, payload } });

  serverSide.onMessage((text) => {
    const message = JSON.parse(text) as { id?: number; method: string; params: unknown };
    const reply = (result: unknown) => send({ id: message.id, result });
    switch (message.method) {
      case "host.info": {
        infoCalls(message.id!);
        if (holdInfo) return;
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
      case "request.cancel":
        cancels.push((message.params as { id: number }).id);
        return;
      case "events.subscribe": {
        const id = nextSubscription++;
        const sub = message.params as { projectId?: string; topics: string[] };
        subscriptions.set(id, sub);
        reply({ subscriptionId: id });
        if (emitRightAfterSubscribe)
          event(id, sub.projectId ?? null, emitRightAfterSubscribe, null);
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
    cancels,
    subscriptions,
    /** `projectId` 가 `null` 이면 Host 범위 이벤트 — `projectId` 없이 연 구독만 받는다(pw-rpc EventHub 와 같은 규칙). */
    emit(projectId: string | null, topic: string, payload: unknown = null) {
      for (const [subscriptionId, sub] of subscriptions) {
        if ((sub.projectId ?? null) !== projectId || !sub.topics.includes(topic)) continue;
        event(subscriptionId, projectId, topic, payload);
      }
    },
  };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

let queryClient: QueryClient;

afterEach(() => queryClient?.clear());

function setup(options?: FakeHostOptions) {
  const host = fakeHost(options);
  queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  // 화면에 붙어 있는 쿼리처럼 observer 를 단다. 무효화는 observer 가 있는 쿼리만 다시 부른다.
  const observer = new QueryObserver(
    queryClient,
    rpcQueryOptions(host.client, HOST, "host.info", null),
  );
  const unsubscribe = observer.subscribe(() => {});
  return { host, observer, unsubscribe };
}

const versionOf = (observer: QueryObserver<HostInfo>) => observer.getCurrentResult().data?.version;

const hostChanged: InvalidationRule = {
  topic: "host.changed",
  keys: [rpcQueryKey(HOST, "host.info")],
};

describe("rpc queries", () => {
  it("keys a query by host, method and params", async () => {
    const { host, observer, unsubscribe } = setup();
    await vi.waitFor(() => expect(versionOf(observer)).toBe("0.0.1"));
    expect(queryClient.getQueryData(rpcQueryKey(HOST, "host.info", null))).toMatchObject({
      hostId: "local",
    });
    expect(host.infoCalls).toHaveBeenCalledTimes(1);
    unsubscribe();
  });

  it("sends request.cancel for the call when the query is cancelled", async () => {
    const { host, unsubscribe } = setup({ holdInfo: true });
    await vi.waitFor(() => expect(host.infoCalls).toHaveBeenCalledTimes(1));
    const requestId = host.infoCalls.mock.calls[0]![0];

    await queryClient.cancelQueries({ queryKey: rpcQueryKey(HOST, "host.info") });
    await vi.waitFor(() => expect(host.cancels).toEqual([requestId]));
    unsubscribe();
  });
});

describe("connectEventInvalidation", () => {
  it("invalidates the rule keys once when the subscription opens", async () => {
    const { host, observer, unsubscribe } = setup();
    await vi.waitFor(() => expect(versionOf(observer)).toBe("0.0.1"));

    const invalidation = connectEventInvalidation(host.client, queryClient, "p1", [hostChanged]);
    await invalidation.ready;
    expect([...host.subscriptions.values()]).toEqual([
      { projectId: "p1", topics: ["host.changed"] },
    ]);
    await vi.waitFor(() => expect(versionOf(observer)).toBe("0.0.2"));
    expect(host.infoCalls).toHaveBeenCalledTimes(2);

    invalidation.dispose();
    unsubscribe();
  });

  it("refetches the queries a matching event invalidates", async () => {
    const { host, observer, unsubscribe } = setup();
    await vi.waitFor(() => expect(versionOf(observer)).toBe("0.0.1"));
    const invalidation = connectEventInvalidation(host.client, queryClient, "p1", [hostChanged]);
    await invalidation.ready;
    await vi.waitFor(() => expect(versionOf(observer)).toBe("0.0.2"));

    host.emit("p1", "host.changed");
    await vi.waitFor(() => expect(versionOf(observer)).toBe("0.0.3"));
    expect(host.infoCalls).toHaveBeenCalledTimes(3);

    invalidation.dispose();
    unsubscribe();
  });

  it("subscribes without a project id for the host scope and refetches on host events", async () => {
    const { host, observer, unsubscribe } = setup();
    await vi.waitFor(() => expect(versionOf(observer)).toBe("0.0.1"));
    const invalidation = connectEventInvalidation(host.client, queryClient, null, [hostChanged]);
    await invalidation.ready;
    expect([...host.subscriptions.values()]).toEqual([{ topics: ["host.changed"] }]);
    await vi.waitFor(() => expect(versionOf(observer)).toBe("0.0.2"));

    host.emit("p1", "host.changed");
    await settle();
    expect(host.infoCalls).toHaveBeenCalledTimes(2);
    host.emit(null, "host.changed");
    await vi.waitFor(() => expect(versionOf(observer)).toBe("0.0.3"));

    invalidation.dispose();
    unsubscribe();
  });

  it("leaves queries alone for topics no rule names", async () => {
    const { host, observer, unsubscribe } = setup();
    await vi.waitFor(() => expect(versionOf(observer)).toBe("0.0.1"));
    const invalidation = connectEventInvalidation(host.client, queryClient, "p1", [
      hostChanged,
      { topic: "fs.changed", keys: [rpcQueryKey(HOST, "fs.tree" as "host.info")] },
    ]);
    await invalidation.ready;
    await vi.waitFor(() => expect(versionOf(observer)).toBe("0.0.2"));

    host.emit("p1", "fs.changed");
    await settle();
    expect(queryClient.getQueryState(rpcQueryKey(HOST, "host.info", null))?.isInvalidated).toBe(
      false,
    );
    expect(host.infoCalls).toHaveBeenCalledTimes(2);
    invalidation.dispose();
    unsubscribe();
  });

  it("lets a rule narrow the keys from the event payload", async () => {
    const { host, observer, unsubscribe } = setup();
    await vi.waitFor(() => expect(versionOf(observer)).toBe("0.0.1"));
    const narrow = vi.fn(() => [rpcQueryKey(HOST, "host.info")]);
    const invalidation = connectEventInvalidation(host.client, queryClient, "p1", [
      { topic: "host.changed", keys: [], narrow },
    ]);
    await invalidation.ready;

    host.emit("p1", "host.changed", { reason: "upgrade" });
    await vi.waitFor(() => expect(versionOf(observer)).toBe("0.0.2"));
    expect(narrow.mock.calls).toEqual([
      [expect.objectContaining({ topic: "host.changed", payload: { reason: "upgrade" } })],
    ]);
    invalidation.dispose();
    unsubscribe();
  });

  it("unsubscribes on dispose and ignores events after it", async () => {
    const { host, observer, unsubscribe } = setup();
    await vi.waitFor(() => expect(versionOf(observer)).toBe("0.0.1"));
    const invalidation = connectEventInvalidation(host.client, queryClient, "p1", [hostChanged]);
    await invalidation.ready;
    await vi.waitFor(() => expect(versionOf(observer)).toBe("0.0.2"));
    invalidation.dispose();
    await vi.waitFor(() => expect(host.unsubscribeCalls).toHaveBeenCalledTimes(1));

    host.emit("p1", "host.changed");
    await settle();
    expect(host.infoCalls).toHaveBeenCalledTimes(2);
    unsubscribe();
  });

  // 구독 응답과 바로 뒤의 이벤트는 따로 도착하고, 클라이언트는 응답을 받는 자리에서 리스너를 단다. 열린 구독을 닫는
  // `.then` 은 그 이벤트보다 늦게 돌아, 그 이벤트는 dispose 된 무효화에 닿는다. 그것을 버리는 것이 dispose 가드다.
  // dispose 뒤의 unsubscribe 는 클라이언트가 보내기 전에 리스너를 떼므로, 응답을 늦추는 시험은 이 가드를 구분하지 못한다.
  it("ignores an event that lands between the subscribe answer and closing a disposed subscription", async () => {
    const { host, observer, unsubscribe } = setup({ emitRightAfterSubscribe: "host.changed" });
    await vi.waitFor(() => expect(versionOf(observer)).toBe("0.0.1"));
    const invalidation = connectEventInvalidation(host.client, queryClient, "p1", [hostChanged]);
    invalidation.dispose();
    await invalidation.ready;
    await vi.waitFor(() => expect(host.unsubscribeCalls).toHaveBeenCalledTimes(1));
    await settle();
    expect(host.subscriptions.size).toBe(0);
    expect(host.infoCalls).toHaveBeenCalledTimes(1);
    unsubscribe();
  });
});
