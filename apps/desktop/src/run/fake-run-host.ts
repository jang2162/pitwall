// 시험용 가짜 Host(실행 쪽): `run.list` 는 시험이 정한 실행, `run.attach` 는 실행마다 시험이 밀어 넣는 항목, `events.subscribe` 는
// 시험이 이벤트를 쏘는 리스너. unary 호출은 기록한다.

import type { EventMessage, RpcClient, RunAttachEvent, RunInfo } from "@pitwall/rpc-client";

export function runInfo(patch: Partial<RunInfo> & { runId: string }): RunInfo {
  return {
    projectId: "p1",
    config: "dev",
    name: "dev",
    kind: "process",
    launchedBy: { kind: "window", window: "main" },
    worktree: null,
    cwd: "/main/app",
    profile: null,
    pid: 100,
    state: "running",
    exit: null,
    startedAt: Date.UTC(2026, 10, 18, 9, 30),
    endedAt: null,
    restartOf: null,
    logStart: 0,
    logEnd: 0,
    ...patch,
  };
}

const b64 = (text: string) => btoa(String.fromCharCode(...new TextEncoder().encode(text)));

export class FakeRunHost {
  runs: RunInfo[] = [];
  readonly calls: { method: string; params: Record<string, unknown> }[] = [];
  readonly attaches: Record<string, unknown>[] = [];
  /** `call` 이 돌려줄 값(메서드별). */
  results: Record<string, (params: Record<string, unknown>) => unknown> = {};
  private listener: ((event: EventMessage) => void) | undefined;
  private readonly queues = new Map<string, (RunAttachEvent | "end")[]>();
  private readonly waiting = new Map<string, () => void>();

  readonly client: Pick<RpcClient, "call" | "stream" | "subscribe"> = {
    call: ((method: string, params: Record<string, unknown>) => {
      this.calls.push({ method, params });
      if (method === "run.list") return Promise.resolve({ runs: this.runs });
      return Promise.resolve(this.results[method]?.(params) ?? null);
    }) as RpcClient["call"],
    stream: ((
      _method: string,
      params: Record<string, unknown>,
      options?: { signal?: AbortSignal },
    ) => {
      this.attaches.push(params);
      const runId = params.runId as string;
      options?.signal?.addEventListener("abort", () => this.push(runId, "end"));
      const next = async (): Promise<IteratorResult<RunAttachEvent>> => {
        const queue = () => this.queues.get(runId) ?? [];
        while (queue().length === 0) await new Promise<void>((r) => this.waiting.set(runId, r));
        const item = queue().shift()!;
        if (item === "end") return { value: undefined, done: true };
        return { value: item, done: false };
      };
      return { [Symbol.asyncIterator]: () => ({ next }) };
    }) as unknown as RpcClient["stream"],
    subscribe: ((_params: unknown, listener: (event: EventMessage) => void) => {
      this.listener = listener;
      return Promise.resolve({ subscriptionId: 1, unsubscribe: async () => {} });
    }) as RpcClient["subscribe"],
  };

  push(runId: string, item: RunAttachEvent | "end") {
    const queue = this.queues.get(runId) ?? [];
    queue.push(item);
    this.queues.set(runId, queue);
    const w = this.waiting.get(runId);
    this.waiting.delete(runId);
    w?.();
  }

  attached(run: RunInfo, from = 0) {
    this.push(run.runId, { kind: "attached", run, from, dropped: 0 });
  }

  output(runId: string, offset: number, text: string) {
    this.push(runId, { kind: "output", offset, data: b64(text) });
  }

  event(topic: string, run: RunInfo) {
    this.listener?.({ subscriptionId: 1, projectId: run.projectId, topic, payload: run as never });
  }
}

/** 마이크로태스크 · 타이머가 돌게 한다. */
export const settle = (ms = 0) => new Promise((r) => setTimeout(r, ms));
