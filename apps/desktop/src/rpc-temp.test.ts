import { describe, expect, it } from "vitest";

import { RpcError, TempRpcClient } from "./rpc-temp";

function fakeTransport() {
  const sent: Record<string, unknown>[] = [];
  return {
    sent,
    transport: {
      send: async (frame: Record<string, unknown>) => {
        sent.push(frame);
      },
    },
  };
}

describe("TempRpcClient", () => {
  it("matches responses to requests by id", async () => {
    const { sent, transport } = fakeTransport();
    const client = new TempRpcClient(transport);
    const a = client.request("host.ping", { nonce: "a" });
    const b = client.request("host.ping", { nonce: "b" });
    expect(sent.map((f) => f.id)).toEqual([1, 2]);

    client.handle({ jsonrpc: "2.0", id: 2, result: "second" });
    client.handle({ jsonrpc: "2.0", id: 1, error: { code: -32601, message: "method not found" } });
    await expect(b).resolves.toBe("second");
    await expect(a).rejects.toBeInstanceOf(RpcError);
  });

  it("grants credit, forwards chunks and resolves on stream.end", async () => {
    const { sent, transport } = fakeTransport();
    const client = new TempRpcClient(transport);
    const items: unknown[] = [];
    const opening = client.stream("host.demoTask", { steps: 2 }, (item) => items.push(item));
    client.handle({ jsonrpc: "2.0", id: 1, result: { streamId: 9 } });
    const stream = await opening;

    expect(sent.at(-1)).toMatchObject({ method: "stream.credit", params: { streamId: 9 } });
    client.handle({ jsonrpc: "2.0", method: "stream.chunk", params: { streamId: 9, data: "x" } });
    expect(items).toEqual(["x"]);
    expect(sent.at(-1)).toEqual({
      jsonrpc: "2.0",
      method: "stream.credit",
      params: { streamId: 9, credit: 1 },
    });

    stream.cancel();
    expect(sent.at(-1)).toMatchObject({ method: "stream.cancel", params: { streamId: 9 } });
    client.handle({
      jsonrpc: "2.0",
      method: "stream.end",
      params: { streamId: 9, status: "cancelled" },
    });
    await expect(stream.done).resolves.toEqual({ status: "cancelled" });
  });
});
