// 시험용 가짜 Host: `terminal.create` 스트림을 시험이 항목 단위로 밀어 넣고, unary 호출(`terminal.input` 등)을 기록한다.
// 스트림은 소비자가 꺼낼 때만 항목을 내준다 — 실제 RPC 클라이언트처럼 소비자가 멈추면 쌓이기만 한다.

import type { RpcClient, TerminalEvent } from "@pitwall/rpc-client";

export interface FakeCall {
  method: string;
  params: Record<string, unknown>;
}

export class FakeTerminalHost {
  readonly calls: FakeCall[] = [];
  readonly creates: Record<string, unknown>[] = [];
  /** 소비자가 지금까지 꺼낸 항목 수. */
  pulled = 0;
  private queue: (TerminalEvent | Error | "end")[] = [];
  private waiting: (() => void) | undefined;
  /** `call` 이 돌려줄 값(메서드별). 없으면 `null`. */
  results: Record<string, (params: Record<string, unknown>) => unknown> = {};
  /** `call` 을 끝내지 않고 붙잡는다(입력 묶기 시험). */
  hold: Set<string> = new Set();
  private held: (() => void)[] = [];
  cancelled = false;

  readonly client: Pick<RpcClient, "call" | "stream"> = {
    call: ((method: string, params: Record<string, unknown>) => {
      this.calls.push({ method, params });
      const result = this.results[method]?.(params) ?? null;
      if (this.hold.has(method)) {
        return new Promise<unknown>((resolve) => this.held.push(() => resolve(result)));
      }
      return result instanceof Error ? Promise.reject(result) : Promise.resolve(result);
    }) as RpcClient["call"],
    stream: ((
      _method: string,
      params: Record<string, unknown>,
      options?: { signal?: AbortSignal },
    ) => {
      this.creates.push(params);
      options?.signal?.addEventListener("abort", () => {
        this.cancelled = true;
        this.push("end");
      });
      const next = async (): Promise<IteratorResult<TerminalEvent>> => {
        while (this.queue.length === 0) await new Promise<void>((r) => (this.waiting = r));
        const item = this.queue.shift()!;
        if (item === "end") return { value: undefined, done: true };
        if (item instanceof Error) throw item;
        this.pulled++;
        return { value: item, done: false };
      };
      return { [Symbol.asyncIterator]: () => ({ next }) };
    }) as unknown as RpcClient["stream"],
  };

  push(item: TerminalEvent | Error | "end") {
    this.queue.push(item);
    const w = this.waiting;
    this.waiting = undefined;
    w?.();
  }

  started(cwd = "/project", terminalId = 7) {
    this.push({ kind: "started", terminalId, pid: 100, shell: ["/bin/sh", "-l"], cwd });
  }

  output(text: string | Uint8Array) {
    const bytes = typeof text === "string" ? new TextEncoder().encode(text) : text;
    let binary = "";
    for (const b of bytes) binary += String.fromCharCode(b);
    this.push({ kind: "output", data: btoa(binary) });
  }

  releaseHeld() {
    const held = this.held.splice(0);
    for (const r of held) r();
  }

  inputs(): string[] {
    return this.calls
      .filter((c) => c.method === "terminal.input")
      .map((c) => c.params.data as string);
  }
}

/** 마이크로태스크 · 타이머가 돌게 한다. */
export async function settle(ms = 0) {
  await new Promise((r) => setTimeout(r, ms));
}
