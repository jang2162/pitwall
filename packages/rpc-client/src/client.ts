import {
  ERROR_CODE,
  PROTOCOL,
  type CancelRequestParams,
  type EventMessage,
  type JsonValue,
  type NotificationMessage,
  type RequestMessage,
  type ResponseMessage,
  type RpcError,
  type StreamCancelParams,
  type StreamChunk,
  type StreamCreditParams,
  type StreamEnd,
  type StreamMethods,
  type StreamOpened,
  type SubscribeParams,
  type Subscribed,
  type UnaryMethods,
  type UnsubscribeParams,
} from "./generated/rpc";
import type { Transport } from "./transport";

/** unary 메서드 맵의 모양. 생성된 `UnaryMethods` 가 이것을 만족한다. */
export type UnaryMap = Record<string, { params: unknown; result: unknown }>;
/** stream 메서드 맵의 모양. 생성된 `StreamMethods` 가 이것을 만족한다. */
export type StreamMap = Record<string, { params: unknown; item: unknown }>;

export interface CallOptions {
  /** 중단하면 `request.cancel` 을 보내고 즉시 `CANCELLED` 로 거부한다. 서버의 늦은 응답은 버린다. */
  signal?: AbortSignal;
}

export interface StreamOptions {
  /**
   * 한 번에 허락하는 청크 수(credit). 열릴 때 이만큼 주고, 소비자가 버퍼를 비웠는데 남은 credit 이 절반 이하면
   * 다시 이만큼으로 채운다. 기본 32.
   */
  window?: number;
  /** 중단하면 `stream.cancel` 을 보낸다. 반복은 오류 없이 끝난다. 이미 중단된 signal 이면 요청을 보내지 않는다. */
  signal?: AbortSignal;
}

// 클라이언트가 보내는 내장 프로토콜 알림의 params. 생성된 타입으로 묶어, Rust 쪽 와이어 이름이 바뀌면 tsc 가 잡는다.
type ClientNotifications = {
  [PROTOCOL.REQUEST_CANCEL]: CancelRequestParams;
  [PROTOCOL.STREAM_CREDIT]: StreamCreditParams;
  [PROTOCOL.STREAM_CANCEL]: StreamCancelParams;
};
type Notify = <M extends keyof ClientNotifications>(
  method: M,
  params: ClientNotifications[M],
) => void;

// 입력이 `null` 일 수 있는 메서드(Rust 의 `()` · `Option`)는 params 를 생략할 수 있다. 생략하면 `null` 을 보낸다.
type Args<P, O> = null extends P ? [params?: P, options?: O] : [params: P, options?: O];

/** 서버가 돌려준 JSON-RPC 오류. */
export class RpcCallError extends Error {
  readonly code: number;
  readonly data: JsonValue | undefined;

  constructor(error: RpcError) {
    super(`${error.message} (${error.code})`);
    this.name = "RpcCallError";
    this.code = error.code;
    this.data = error.data;
  }
}

/** Host 의 「바쁨」 사유 문구(crates/pw-fs: `search.files` 는 `file search is busy: …`, `search.text` 는 `search is busy: …`). */
const BUSY_DETAIL = /^(?:file )?search is busy\b/;

/**
 * Host 가 동시 상한에 걸려 요청을 거절했다(「바쁨」, `search.files` · `search.text`). 잠시 뒤 다시 보내면 된다.
 * Host 는 바쁨을 `RpcError::internal(사유)` 로 보낸다 — 와이어의 `message` 는 「internal error」 이고 사유는 `data` 문자열에 있다
 * (crates/pw-rpc/src/error.rs). 그래서 `message` 가 아니라 `code` 와 `data` 로 가른다. Host 가 바쁨 전용 오류 코드를 두면(M1-07 의
 * 이월) 그 코드로 가른다.
 */
export function isBusyError(error: unknown): boolean {
  return (
    error instanceof RpcCallError &&
    error.code === ERROR_CODE.INTERNAL_ERROR &&
    typeof error.data === "string" &&
    BUSY_DETAIL.test(error.data)
  );
}

const cancelledError = () => new RpcCallError({ code: ERROR_CODE.CANCELLED, message: "cancelled" });

export interface Subscription {
  readonly id: number;
  /** 즉시 리스너를 떼고 서버에 `events.unsubscribe` 를 보낸다. */
  unsubscribe(): Promise<void>;
}

type Pending =
  | { kind: "unary"; resolve: (value: unknown) => void; reject: (error: unknown) => void }
  | { kind: "stream"; stream: StreamState<unknown> }
  | {
      kind: "subscribe";
      listener: (event: EventMessage) => void;
      resolve: (subscription: Subscription) => void;
      reject: (error: unknown) => void;
    };

/**
 * transport 에 의존하지 않는 JSON-RPC 클라이언트. 메서드 맵 타입으로 이름 · 입력 · 결과를 검사한다.
 *
 * 응답과 그 뒤에 바로 오는 알림(스트림 청크 · 이벤트)의 경합을 막으려고, 스트림 · 구독의 응답은 수신 처리 안에서
 * 동기적으로 등록한다 — Promise 가 풀리기를 기다리면 그 사이에 도착한 알림을 놓친다.
 *
 * transport 가 닫히면(`Transport.onClose`) `close()` 와 같이 정리한다. `send` 가 던지면 그 호출만 오류로 끝낸다.
 */
export class RpcClient<U extends UnaryMap = UnaryMethods, S extends StreamMap = StreamMethods> {
  private readonly transport: Transport;
  private readonly detach: () => void;
  private readonly detachClose: () => void;
  private nextId = 1;
  private readonly pending = new Map<number, Pending>();
  private readonly streams = new Map<number, StreamState<unknown>>();
  private readonly subscriptions = new Map<number, (event: EventMessage) => void>();
  private closed = false;

  constructor(transport: Transport) {
    this.transport = transport;
    this.detach = transport.onMessage((text) => this.receive(text));
    this.detachClose =
      transport.onClose?.(() => this.teardown(new Error("rpc transport closed"))) ?? (() => {});
  }

  /** unary 메서드를 부른다. */
  call<K extends keyof U & string>(
    method: K,
    ...[params, options]: Args<U[K]["params"], CallOptions>
  ): Promise<U[K]["result"]> {
    return new Promise((resolve, reject) => {
      const signal = options?.signal;
      if (signal?.aborted) {
        reject(cancelledError());
        return;
      }
      const onAbort = () => {
        if (!this.pending.delete(id)) return;
        try {
          this.notify(PROTOCOL.REQUEST_CANCEL, { id });
        } catch {
          // 보내지 못해도 호출자에게는 취소로 끝난다. 서버의 늦은 응답은 대기 항목이 없어 버려진다.
        }
        reject(cancelledError());
      };
      // 응답 뒤에도 리스너가 남으면, 오래 사는 signal 하나를 여러 호출에 쓸 때 클로저가 쌓인다.
      const release = () => signal?.removeEventListener("abort", onAbort);
      const id = this.request(method, params, {
        kind: "unary",
        resolve: (value) => {
          release();
          resolve(value as U[K]["result"]);
        },
        reject: (error) => {
          release();
          reject(error);
        },
      });
      if (this.pending.has(id)) signal?.addEventListener("abort", onAbort, { once: true });
    });
  }

  /** stream 메서드를 부른다. 돌려준 객체를 `for await` 로 읽는다. 중간에 `break` 하면 스트림을 취소한다. */
  stream<K extends keyof S & string>(
    method: K,
    ...[params, options]: Args<S[K]["params"], StreamOptions>
  ): RpcStream<S[K]["item"]> {
    const state = new StreamState<S[K]["item"]>(
      (method, params) => this.notify(method, params),
      options?.window ?? 32,
    );
    const signal = options?.signal;
    if (signal?.aborted) {
      // 시작 뒤 중단과 같게 오류 없이 끝난다. 요청은 보내지 않는다.
      state.cancel();
      return new RpcStream(state);
    }
    this.request(method, params, { kind: "stream", stream: state as StreamState<unknown> });
    if (signal && !state.stopped) {
      const onAbort = () => state.cancel();
      signal.addEventListener("abort", onAbort, { once: true });
      state.onStop(() => signal.removeEventListener("abort", onAbort));
    }
    return new RpcStream(state);
  }

  /** `events.subscribe`. 토픽은 정확히 일치해야 한다. */
  subscribe(
    params: SubscribeParams,
    listener: (event: EventMessage) => void,
  ): Promise<Subscription> {
    return new Promise((resolve, reject) => {
      this.request(PROTOCOL.EVENTS_SUBSCRIBE, params, {
        kind: "subscribe",
        listener,
        resolve,
        reject,
      });
    });
  }

  /** 리스너를 떼고, 진행 중인 요청을 거부하고, 스트림을 끝낸다. 서버에는 아무것도 보내지 않는다. */
  close(): void {
    this.teardown(new Error("rpc client closed"));
  }

  private teardown(error: Error): void {
    if (this.closed) return;
    this.closed = true;
    this.detach();
    this.detachClose();
    const pending = [...this.pending.values()];
    const streams = [...this.streams.values()];
    this.pending.clear();
    this.streams.clear();
    this.subscriptions.clear();
    for (const p of pending) settleWithError(p, error);
    for (const stream of streams) stream.fail(error);
  }

  private readonly notify: Notify = (method, params) => {
    this.send({ jsonrpc: "2.0", method, params });
  };

  private request(method: string, params: unknown, pending: Pending): number {
    const id = this.nextId++;
    if (this.closed) {
      settleWithError(pending, new Error("rpc client closed"));
      return id;
    }
    this.pending.set(id, pending);
    try {
      this.send({ jsonrpc: "2.0", id, method, params: (params ?? null) as JsonValue });
    } catch (error) {
      this.pending.delete(id);
      settleWithError(pending, error);
    }
    return id;
  }

  private send(message: RequestMessage): void {
    this.transport.send(JSON.stringify(message));
  }

  private receive(text: string): void {
    let message: ResponseMessage | NotificationMessage;
    try {
      message = JSON.parse(text);
    } catch {
      return;
    }
    // 객체가 아닌 프레임에 `in` 을 쓰면 TypeError 가 transport 리스너 밖으로 던져진다.
    if (typeof message !== "object" || message === null || Array.isArray(message)) return;
    if ("method" in message) this.onNotification(message);
    else this.onResponse(message);
  }

  private onResponse(message: ResponseMessage): void {
    if (typeof message.id !== "number") return;
    const pending = this.pending.get(message.id);
    if (!pending) return;
    this.pending.delete(message.id);
    const error = "error" in message && message.error ? new RpcCallError(message.error) : undefined;
    const result = "result" in message ? message.result : null;
    switch (pending.kind) {
      case "unary":
        if (error) pending.reject(error);
        else pending.resolve(result);
        return;
      case "stream":
        if (error) pending.stream.fail(error);
        else {
          const { streamId } = result as StreamOpened;
          this.streams.set(streamId, pending.stream);
          // 서버의 stream.end 없이 끝나는 경우(credit · cancel 전송 실패)에도 표에서 걷는다.
          pending.stream.onFinish(() => this.streams.delete(streamId));
          pending.stream.open(streamId);
        }
        return;
      case "subscribe": {
        if (error) {
          pending.reject(error);
          return;
        }
        const { subscriptionId } = result as Subscribed;
        this.subscriptions.set(subscriptionId, pending.listener);
        pending.resolve({
          id: subscriptionId,
          unsubscribe: async () => {
            if (!this.subscriptions.delete(subscriptionId)) return;
            await this.unsubscribe(subscriptionId);
          },
        });
      }
    }
  }

  private unsubscribe(subscriptionId: number): Promise<void> {
    return new Promise((resolve, reject) => {
      this.request(PROTOCOL.EVENTS_UNSUBSCRIBE, { subscriptionId } satisfies UnsubscribeParams, {
        kind: "unary",
        resolve: () => resolve(),
        reject,
      });
    });
  }

  private onNotification(message: NotificationMessage): void {
    switch (message.method) {
      case PROTOCOL.STREAM_CHUNK: {
        const chunk = message.params as StreamChunk;
        this.streams.get(chunk.streamId)?.push(chunk.data);
        return;
      }
      case PROTOCOL.STREAM_END: {
        const end = message.params as StreamEnd;
        const stream = this.streams.get(end.streamId);
        this.streams.delete(end.streamId);
        stream?.end(end);
        return;
      }
      case PROTOCOL.EVENTS_EVENT: {
        const event = message.params as EventMessage;
        this.subscriptions.get(event.subscriptionId)?.(event);
      }
    }
  }
}

function settleWithError(pending: Pending, error: unknown): void {
  if (pending.kind === "stream") pending.stream.fail(error);
  else pending.reject(error);
}

type Outcome = { done: true } | { error: unknown };

/** 스트림 하나의 수신 상태. 청크는 `window` 만큼 credit 을 준 뒤에만 온다. */
class StreamState<T> {
  private readonly notify: Notify;
  private readonly window: number;
  private streamId: number | undefined;
  private granted = 0;
  private received = 0;
  private cancelRequested = false;
  private outcome: Outcome | undefined;
  private readonly buffer: T[] = [];
  private readonly stopHooks: (() => void)[] = [];
  private readonly finishHooks: (() => void)[] = [];
  private waiter:
    | { resolve: (r: IteratorResult<T>) => void; reject: (e: unknown) => void }
    | undefined;

  constructor(notify: Notify, window: number) {
    this.notify = notify;
    this.window = Math.max(1, Math.floor(window));
  }

  /** 끝났거나 소비자가 취소했다. */
  get stopped(): boolean {
    return this.outcome !== undefined || this.cancelRequested;
  }

  /** 끝나거나 취소될 때 한 번 부른다. */
  onStop(hook: () => void): void {
    if (this.stopped) hook();
    else this.stopHooks.push(hook);
  }

  /** 끝날 때(서버의 stream.end, 오류, 닫힘) 한 번 부른다. */
  onFinish(hook: () => void): void {
    if (this.outcome) hook();
    else this.finishHooks.push(hook);
  }

  open(streamId: number): void {
    this.streamId = streamId;
    if (this.cancelRequested) this.sendCancel();
    else this.grant(this.window);
  }

  push(data: unknown): void {
    this.received++;
    const item = data as T;
    if (this.waiter) {
      const { resolve } = this.waiter;
      this.waiter = undefined;
      resolve({ value: item, done: false });
    } else this.buffer.push(item);
  }

  end(end: Pick<StreamEnd, "status"> & { error?: RpcError }): void {
    if (end.status === "completed") this.finish({ done: true });
    else if (end.status === "failed" && end.error)
      this.finish({ error: new RpcCallError(end.error) });
    // 우리가 취소했으면 정상 종료다. 아니면 서버가 세션을 닫은 것이라 소비자에게 알린다.
    else this.finish(this.cancelRequested ? { done: true } : { error: cancelledError() });
  }

  fail(error: unknown): void {
    this.finish({ error });
  }

  cancel(): void {
    if (this.stopped) return;
    this.cancelRequested = true;
    this.runStopHooks();
    // 취소 뒤 버퍼의 청크는 버리고 반복을 끝낸다. 서버의 stream.end(cancelled)는 상태 정리에만 쓰인다.
    this.buffer.length = 0;
    if (this.waiter) {
      const { resolve } = this.waiter;
      this.waiter = undefined;
      resolve({ value: undefined, done: true });
    }
    this.sendCancel();
  }

  next(): Promise<IteratorResult<T>> {
    if (this.buffer.length > 0)
      return Promise.resolve({ value: this.buffer.shift() as T, done: false });
    if (this.cancelRequested) return Promise.resolve({ value: undefined, done: true });
    if (this.outcome) {
      return "error" in this.outcome
        ? Promise.reject(this.outcome.error)
        : Promise.resolve({ value: undefined, done: true });
    }
    // 버퍼가 빈 채로 기다릴 때만 credit 을 채운다. 소비자가 읽지 않는 동안에는 window 이상 쌓이지 않는다.
    const outstanding = this.granted - this.received;
    if (this.streamId !== undefined && outstanding <= Math.floor(this.window / 2)) {
      this.grant(this.window - outstanding);
    }
    return new Promise((resolve, reject) => {
      this.waiter = { resolve, reject };
    });
  }

  private finish(outcome: Outcome): void {
    if (this.outcome) return;
    this.outcome = outcome;
    this.runStopHooks();
    for (const hook of this.finishHooks.splice(0)) hook();
    const waiter = this.waiter;
    if (!waiter) return;
    this.waiter = undefined;
    if ("error" in outcome) waiter.reject(outcome.error);
    else waiter.resolve({ value: undefined, done: true });
  }

  private runStopHooks(): void {
    for (const hook of this.stopHooks.splice(0)) hook();
  }

  private grant(credit: number): void {
    if (credit <= 0 || this.streamId === undefined) return;
    try {
      this.notify(PROTOCOL.STREAM_CREDIT, { streamId: this.streamId, credit });
    } catch (error) {
      // credit 이 안 가면 청크도 오지 않는다. 기다리게 두지 않고 그 오류로 끝낸다.
      this.fail(error);
      return;
    }
    this.granted += credit;
  }

  private sendCancel(): void {
    if (this.streamId === undefined) return;
    try {
      this.notify(PROTOCOL.STREAM_CANCEL, { streamId: this.streamId });
    } catch {
      // 반복은 이미 끝났다. 서버의 stream.end 가 오지 않을 것이므로 여기서 상태를 닫는다.
      this.finish({ done: true });
    }
  }
}

/** 스트림 메서드의 결과. 한 소비자만 읽는다. */
export class RpcStream<T> implements AsyncIterable<T> {
  private readonly state: StreamState<T>;

  /** @internal */
  constructor(state: StreamState<T>) {
    this.state = state;
  }

  /** `stream.cancel` 을 보내고 반복을 끝낸다. 이미 끝났으면 아무것도 하지 않는다. */
  cancel(): void {
    this.state.cancel();
  }

  [Symbol.asyncIterator](): AsyncIterator<T> {
    return {
      next: () => this.state.next(),
      return: async () => {
        this.state.cancel();
        return { value: undefined, done: true };
      },
    };
  }
}
