// 임시 RPC 클라이언트: UI ↔ LocalHost 왕복 확인용(M0-07).
// M0-06 의 rpc-client(packages/rpc-client)로 교체 예정이다. 교체할 때 이 파일과 host-demo.tsx 를 함께 지운다.
//
// transport 는 src-tauri/src/ipc.rs: rpc_connect 로 연결을 열고, rpc_send 로 JSON-RPC 메시지를 보내고,
// Host 가 보내는 메시지는 rpc_connect 에 넘긴 Channel 로 받는다.

import { Channel, invoke } from "@tauri-apps/api/core";

type Frame = Record<string, unknown>;

export interface Transport {
  send(frame: Frame): Promise<unknown>;
}

export class RpcError extends Error {
  constructor(
    readonly code: number,
    message: string,
    readonly data?: unknown,
  ) {
    super(message);
  }
}

export type StreamEnd =
  | { status: "completed" }
  | { status: "failed"; error: { code: number; message: string } }
  | { status: "cancelled" };

interface OpenStream {
  onItem: (item: unknown) => void;
  finish: (end: StreamEnd) => void;
}

export interface StreamHandle {
  streamId: number;
  cancel(): void;
  done: Promise<StreamEnd>;
}

/** 한 번에 내주는 credit. 청크를 하나 받을 때마다 1 씩 다시 채운다. */
const CREDIT_WINDOW = 16;

export class TempRpcClient {
  private nextId = 1;
  private readonly pending = new Map<
    number,
    { resolve: (v: unknown) => void; reject: (e: RpcError) => void }
  >();
  private readonly streams = new Map<number, OpenStream>();

  constructor(private readonly transport: Transport) {}

  request<T>(method: string, params?: unknown): Promise<T> {
    const id = this.nextId++;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
      this.transport
        .send({ jsonrpc: "2.0", id, method, params: params ?? null })
        .catch((e: unknown) => {
          this.pending.delete(id);
          reject(new RpcError(-1, `transport: ${JSON.stringify(e)}`));
        });
    });
  }

  async stream(
    method: string,
    params: unknown,
    onItem: (item: unknown) => void,
  ): Promise<StreamHandle> {
    const { streamId } = await this.request<{ streamId: number }>(method, params);
    let finish!: (end: StreamEnd) => void;
    const done = new Promise<StreamEnd>((resolve) => {
      finish = resolve;
    });
    this.streams.set(streamId, { onItem, finish });
    this.notify("stream.credit", { streamId, credit: CREDIT_WINDOW });
    return {
      streamId,
      cancel: () => this.notify("stream.cancel", { streamId }),
      done,
    };
  }

  /** Channel 로 받은 메시지 하나를 처리한다. */
  handle(frame: Frame): void {
    if (typeof frame.id === "number" && !("method" in frame)) {
      const waiter = this.pending.get(frame.id);
      if (!waiter) return;
      this.pending.delete(frame.id);
      const error = frame.error as { code: number; message: string; data?: unknown } | undefined;
      if (error) waiter.reject(new RpcError(error.code, error.message, error.data));
      else waiter.resolve(frame.result);
      return;
    }
    const params = frame.params as { streamId: number; data?: unknown } & Partial<StreamEnd>;
    if (frame.method === "stream.chunk") {
      const stream = this.streams.get(params.streamId);
      if (!stream) return;
      stream.onItem(params.data);
      this.notify("stream.credit", { streamId: params.streamId, credit: 1 });
    } else if (frame.method === "stream.end") {
      const stream = this.streams.get(params.streamId);
      if (!stream) return;
      this.streams.delete(params.streamId);
      const { streamId: _, ...end } = params;
      stream.finish(end as StreamEnd);
    }
  }

  private notify(method: string, params: unknown): void {
    void this.transport.send({ jsonrpc: "2.0", method, params }).catch(() => {});
  }
}

export interface TempConnection {
  client: TempRpcClient;
  close(): Promise<void>;
}

/** 데스크톱 프로세스 안의 Host(src-tauri/src/main.rs 의 LOCAL_HOST_ID)에 연결한다. */
export async function connectLocalHost(): Promise<TempConnection> {
  let conn: number | undefined;
  const client = new TempRpcClient({
    send: (frame) => invoke("rpc_send", { conn, frame }),
  });
  const channel = new Channel<Frame>((frame) => client.handle(frame));
  conn = await invoke<number>("rpc_connect", { target: { hostId: "local" }, channel });
  return {
    client,
    close: () => invoke("rpc_disconnect", { conn }),
  };
}
