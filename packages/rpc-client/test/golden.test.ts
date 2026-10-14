// Rust 의 실제 pw-rpc Session 이 주고받은 메시지(fixtures/golden.json)를 재생한다. 클라이언트가 보내는 JSON 은
// Rust 가 기대한 것과 같아야 하고, 서버가 보낸 JSON 은 클라이언트가 읽어 타입대로 돌려줘야 한다.
// 호출 순서는 crates/pw-rpc-gen/src/bin/pw-rpc-gen-fixtures.rs 의 record() 와 같다.
import { describe, expect, it } from "vitest";

import { RpcClient, type Transport } from "../src";
import { ERROR_CODE } from "../src/generated/rpc";
import golden from "./fixtures/golden.json";
import type { EventMessage, StreamMethods, UnaryMethods } from "./fixtures/rpc";

type Exchange = { from: string; message: unknown };

/** 클라이언트가 보낸 메시지를 다음 골든 메시지와 비교하고, 그 뒤의 서버 메시지들을 돌려준다. */
class ReplayTransport implements Transport {
  private index = 0;
  private readonly listeners = new Set<(text: string) => void>();
  private readonly exchanges: Exchange[];

  constructor(exchanges: Exchange[]) {
    this.exchanges = exchanges;
  }

  send(text: string): void {
    const expected = this.exchanges[this.index];
    expect(expected?.from, `unexpected client message: ${text}`).toBe("client");
    expect(JSON.parse(text)).toEqual(expected?.message);
    this.index++;
    queueMicrotask(() => this.deliverServerMessages());
  }

  onMessage(listener: (text: string) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  remaining(): Exchange[] {
    return this.exchanges.slice(this.index);
  }

  private deliverServerMessages(): void {
    while (this.exchanges[this.index]?.from === "server") {
      const text = JSON.stringify(this.exchanges[this.index]?.message);
      this.index++;
      for (const listener of this.listeners) listener(text);
    }
  }
}

describe("golden exchange with the Rust session", () => {
  it("sends and parses exactly what pw-rpc does", async () => {
    const replay = new ReplayTransport(golden.exchanges);
    const client = new RpcClient<UnaryMethods, StreamMethods>(replay);

    const echoed = await client.call("fixture.echo", {
      text: "hi",
      repeatCount: 2,
      mode: "upper",
      note: null,
    });
    expect(echoed).toEqual({ text: "HIHI", length: 4 });

    const items: number[] = [];
    for await (const item of client.stream("fixture.count", { n: 3 }, { window: 3 }))
      items.push(item);
    expect(items).toEqual([0, 1, 2]);

    await expect(client.call("fixture.fail")).rejects.toMatchObject({
      code: 1001,
      message: "boom (1001)",
      data: { reason: "fixture" },
    });

    const events: EventMessage[] = [];
    const subscription = await client.subscribe({ projectId: "p1", topics: ["fs.changed"] }, (e) =>
      events.push(e),
    );
    expect(events).toEqual([
      { subscriptionId: 1, projectId: "p1", topic: "fs.changed", payload: { path: "src/main.rs" } },
    ]);
    await subscription.unsubscribe();

    const abort = new AbortController();
    const waiting = client.call("fixture.wait", null, { signal: abort.signal });
    abort.abort();
    await expect(waiting).rejects.toMatchObject({ code: ERROR_CODE.CANCELLED });

    for await (const item of client.stream("fixture.count", { n: 100 }, { window: 1 })) {
      expect(item).toBe(0);
      break;
    }

    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(replay.remaining()).toEqual([]);
  });
});
