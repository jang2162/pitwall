// 클라이언트의 동작(credit 보충, 취소, 구독 해제, 닫기)을 in-memory transport 와 손으로 움직이는 서버로 본다.
// 와이어 형식이 Rust 와 같은지는 golden.test.ts 가 본다.
import { describe, expect, it } from "vitest";

import { createMemoryTransportPair, RpcCallError, RpcClient, type Transport } from "../src";
import { ERROR_CODE } from "../src/generated/rpc";
import type { StreamMethods, UnaryMethods } from "./fixtures/rpc";

type Message = { id?: number; method?: string; params?: any };

/** 받은 메시지를 쌓아 두고 테스트가 직접 답하는 서버. */
class FakeServer {
  readonly received: Message[] = [];
  private readonly transport: Transport;

  constructor(transport: Transport) {
    this.transport = transport;
    transport.onMessage((text) => this.received.push(JSON.parse(text)));
  }

  reply(id: number, result: unknown): void {
    this.transport.send(JSON.stringify({ jsonrpc: "2.0", id, result }));
  }

  fail(id: number, code: number, message: string): void {
    this.transport.send(JSON.stringify({ jsonrpc: "2.0", id, error: { code, message } }));
  }

  notify(method: string, params: unknown): void {
    this.transport.send(JSON.stringify({ jsonrpc: "2.0", method, params }));
  }

  /** 지금까지 받은 알림 중 `method` 의 params. */
  notifications(method: string): any[] {
    return this.received
      .filter((m) => m.id === undefined && m.method === method)
      .map((m) => m.params);
  }
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

function setup() {
  const [clientSide, serverSide] = createMemoryTransportPair();
  return {
    client: new RpcClient<UnaryMethods, StreamMethods>(clientSide),
    server: new FakeServer(serverSide),
  };
}

describe("RpcClient", () => {
  it("sends null params when the method takes none and resolves the result", async () => {
    const { client, server } = setup();
    const call = client.call("fixture.fail");
    await settle();
    expect(server.received).toEqual([
      { jsonrpc: "2.0", id: 1, method: "fixture.fail", params: null },
    ]);
    server.reply(1, null);
    await expect(call).resolves.toBeNull();
  });

  it("rejects with RpcCallError carrying the server error", async () => {
    const { client, server } = setup();
    const call = client.call("fixture.fail");
    await settle();
    server.fail(1, ERROR_CODE.PERMISSION_DENIED, "permission denied");
    const error = await call.catch((e: unknown) => e);
    expect(error).toBeInstanceOf(RpcCallError);
    expect(error).toMatchObject({ code: ERROR_CODE.PERMISSION_DENIED });
  });

  it("refills credit only when the consumer has drained the buffer", async () => {
    const { client, server } = setup();
    const stream = client.stream("fixture.count", { n: 10 }, { window: 4 });
    const iterator = stream[Symbol.asyncIterator]();
    await settle();
    server.reply(1, { streamId: 9 });
    await settle();
    expect(server.notifications("stream.credit")).toEqual([{ streamId: 9, credit: 4 }]);

    for (const data of [0, 1, 2]) server.notify("stream.chunk", { streamId: 9, data });
    await settle();
    expect(await iterator.next()).toEqual({ value: 0, done: false });
    expect(await iterator.next()).toEqual({ value: 1, done: false });
    expect(await iterator.next()).toEqual({ value: 2, done: false });
    // 아직 credit 1 이 남아 있다(4 - 3). 버퍼가 비었고 남은 credit 이 window/2 이하라 3 을 채운다.
    expect(server.notifications("stream.credit")).toHaveLength(1);
    const pending = iterator.next();
    await settle();
    expect(server.notifications("stream.credit")).toEqual([
      { streamId: 9, credit: 4 },
      { streamId: 9, credit: 3 },
    ]);
    server.notify("stream.chunk", { streamId: 9, data: 3 });
    expect(await pending).toEqual({ value: 3, done: false });
    server.notify("stream.end", { streamId: 9, status: "completed" });
    expect(await iterator.next()).toEqual({ value: undefined, done: true });
  });

  it("throws the stream failure from the iterator", async () => {
    const { client, server } = setup();
    const stream = client.stream("fixture.count", { n: 1 });
    await settle();
    server.reply(1, { streamId: 1 });
    server.notify("stream.end", {
      streamId: 1,
      status: "failed",
      error: { code: 1001, message: "stream boom" },
    });
    const consume = async () => {
      for await (const _ of stream) void _;
    };
    await expect(consume()).rejects.toMatchObject({ code: 1001 });
  });

  it("cancels a stream that is cancelled before it opened", async () => {
    const { client, server } = setup();
    const stream = client.stream("fixture.count", { n: 1 });
    stream.cancel();
    await settle();
    server.reply(1, { streamId: 5 });
    await settle();
    expect(server.notifications("stream.credit")).toEqual([]);
    expect(server.notifications("stream.cancel")).toEqual([{ streamId: 5 }]);
    expect(await stream[Symbol.asyncIterator]().next()).toEqual({ value: undefined, done: true });
  });

  it("stops delivering events right after unsubscribe", async () => {
    const { client, server } = setup();
    const seen: unknown[] = [];
    const subscribing = client.subscribe({ projectId: "p", topics: ["t"] }, (e) =>
      seen.push(e.payload),
    );
    await settle();
    server.reply(1, { subscriptionId: 3 });
    server.notify("events.event", { subscriptionId: 3, projectId: "p", topic: "t", payload: 1 });
    const subscription = await subscribing;
    const unsubscribing = subscription.unsubscribe();
    server.notify("events.event", { subscriptionId: 3, projectId: "p", topic: "t", payload: 2 });
    await settle();
    expect(seen).toEqual([1]);
    expect(server.received.at(-1)).toMatchObject({
      id: 2,
      method: "events.unsubscribe",
      params: { subscriptionId: 3 },
    });
    server.reply(2, null);
    await unsubscribing;
  });

  it("rejects pending calls and ends streams on close", async () => {
    const { client, server } = setup();
    const call = client.call("fixture.wait");
    const stream = client.stream("fixture.count", { n: 1 });
    await settle();
    server.reply(2, { streamId: 1 });
    await settle();
    client.close();
    await expect(call).rejects.toThrow("closed");
    await expect(stream[Symbol.asyncIterator]().next()).rejects.toThrow("closed");
  });

  it("does not send a request whose signal is already aborted", async () => {
    const { client, server } = setup();
    const abort = new AbortController();
    abort.abort();
    await expect(client.call("fixture.wait", null, { signal: abort.signal })).rejects.toMatchObject(
      {
        code: ERROR_CODE.CANCELLED,
      },
    );
    await settle();
    expect(server.received).toEqual([]);
  });
});
