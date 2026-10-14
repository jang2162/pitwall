import { RpcClient } from "@pitwall/rpc-client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { connectHost, setLeavingForTest } from "./tauri-transport";

// Tauri 의 IPC 는 웹뷰 밖에서 돌지 않는다. invoke 와 Channel 을 바꿔 끼워 transport 의 동작만 본다.
const ipc = vi.hoisted(() => {
  const channels: { onmessage: (frame: unknown) => void }[] = [];
  const calls: { cmd: string; args: Record<string, unknown> }[] = [];
  let respond: (cmd: string) => Promise<unknown> = async (cmd) =>
    cmd === "rpc_connect" ? 7 : null;
  return {
    channels,
    calls,
    setResponder(fn: (cmd: string) => Promise<unknown>) {
      respond = fn;
    },
    invoke(cmd: string, args: Record<string, unknown>) {
      calls.push({ cmd, args });
      return respond(cmd);
    },
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {
    constructor(onmessage: (frame: unknown) => void) {
      ipc.channels.push({ onmessage });
    }
  },
  invoke: ipc.invoke,
}));

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const sent = () => ipc.calls.filter((c) => c.cmd === "rpc_send").map((c) => c.args.frame);

beforeEach(() => {
  ipc.channels.length = 0;
  ipc.calls.length = 0;
  ipc.setResponder(async (cmd) => (cmd === "rpc_connect" ? 7 : null));
  setLeavingForTest(false);
});

afterEach(() => setLeavingForTest(false));

describe("Tauri IPC transport", () => {
  it("round-trips a typed call through rpc_send and the channel", async () => {
    const conn = await connectHost({ hostId: "local" });
    expect(ipc.calls[0]).toMatchObject({
      cmd: "rpc_connect",
      args: { target: { hostId: "local" } },
    });
    const client = new RpcClient(conn.transport);

    const pong = client.call("host.ping", { nonce: "n1" });
    await flush();
    expect(sent()).toEqual([
      { jsonrpc: "2.0", id: 1, method: "host.ping", params: { nonce: "n1" } },
    ]);
    expect(ipc.calls.at(-1)?.args.conn).toBe(7);

    ipc.channels[0]!.onmessage({ jsonrpc: "2.0", id: 1, result: { nonce: "n1", hostId: "local" } });
    await expect(pong).resolves.toEqual({ nonce: "n1", hostId: "local" });
  });

  it("sends frames one at a time in order", async () => {
    const pending: (() => void)[] = [];
    ipc.setResponder((cmd) =>
      cmd === "rpc_connect"
        ? Promise.resolve(7)
        : new Promise((resolve) => pending.push(() => resolve(null))),
    );
    const conn = await connectHost({ hostId: "local" });
    conn.transport.send('{"n":1}');
    conn.transport.send('{"n":2}');
    await flush();
    expect(sent()).toEqual([{ n: 1 }]);
    pending.shift()!();
    await flush();
    expect(sent()).toEqual([{ n: 1 }, { n: 2 }]);
  });

  it("reports a failed rpc_send as a closed transport and fails pending calls", async () => {
    ipc.setResponder(async (cmd) => {
      if (cmd === "rpc_send") throw { kind: "unknownConnection", conn: 7 };
      return cmd === "rpc_connect" ? 7 : null;
    });
    const conn = await connectHost({ hostId: "local" });
    const client = new RpcClient(conn.transport);
    const closed = vi.fn();
    conn.transport.onClose!(closed);

    await expect(client.call("host.info")).rejects.toThrow("rpc transport closed");
    expect(closed).toHaveBeenCalledTimes(1);
    expect(() => conn.transport.send("{}")).toThrow("closed");
  });

  it("close disconnects once and notifies close listeners", async () => {
    const conn = await connectHost({ hostId: "local" });
    const closed = vi.fn();
    conn.transport.onClose!(closed);
    await conn.close();
    await conn.close();
    expect(ipc.calls.filter((c) => c.cmd === "rpc_disconnect")).toEqual([
      { cmd: "rpc_disconnect", args: { conn: 7 } },
    ]);
    expect(closed).toHaveBeenCalledTimes(1);
  });

  it("does not open connections once the page is unloading", async () => {
    setLeavingForTest(true);
    await expect(connectHost({ hostId: "local" })).rejects.toThrow("unloading");
    expect(ipc.calls).toEqual([]);
  });

  it("closes a connection that finished opening after unloading began", async () => {
    ipc.setResponder(async (cmd) => {
      if (cmd === "rpc_connect") setLeavingForTest(true);
      return cmd === "rpc_connect" ? 7 : null;
    });
    await expect(connectHost({ hostId: "local" })).rejects.toThrow("unloading");
    expect(ipc.calls.map((c) => c.cmd)).toEqual(["rpc_connect", "rpc_disconnect"]);
  });
});
