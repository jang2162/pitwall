// `@pitwall/rpc-client` 의 Transport 를 Tauri IPC 로 구현한다. Rust 쪽은 src-tauri/src/ipc.rs.
//
// - rpc_connect({ target, channel }) 가 Host 연결을 열고 연결 id 를 돌려준다. Host 가 보내는 메시지는 channel 로 온다.
// - rpc_send({ conn, frame }) 로 JSON-RPC 메시지 하나를 보낸다. frame 은 JSON 객체다(IPC 가 직렬화한다).
// - rpc_disconnect({ conn }) 로 닫는다.

import type { Transport } from "@pitwall/rpc-client";
import { Channel, invoke } from "@tauri-apps/api/core";

export type HostTarget = { hostId: string } | { projectId: string };

export interface TauriConnection {
  transport: Transport;
  /** 연결을 닫는다. Host 는 진행 중 요청 · 스트림을 취소하고 구독을 걷는다. `onClose` 리스너가 불린다. */
  close(): Promise<void>;
}

// 페이지가 떠나기 시작했는가. 리로드가 시작된 뒤(Rust 의 PageLoadEvent::Started) 새 문서가 커밋되기 전까지 옛 페이지의
// JS 가 계속 돈다. 그 사이 옛 페이지가 연 연결은 Rust 가 새 페이지의 것과 가를 수 없다(ipc.rs 모듈 주석).
// beforeunload 는 탐색이 시작되기 전에 오므로, 여기서부터 새 연결을 열지 않으면 그 틈이 생기지 않는다.
let leaving = false;

/** 테스트에서 페이지 이탈을 흉내 내거나 되돌린다. */
export function setLeavingForTest(value: boolean): void {
  leaving = value;
}

if (typeof window !== "undefined") {
  window.addEventListener("beforeunload", () => {
    leaving = true;
  });
}

export async function connectHost(target: HostTarget): Promise<TauriConnection> {
  if (leaving) throw new Error("page is unloading");
  const messageListeners = new Set<(text: string) => void>();
  const closeListeners = new Set<() => void>();
  let closed = false;
  const markClosed = () => {
    if (closed) return;
    closed = true;
    for (const listener of closeListeners) listener();
  };

  const channel = new Channel<unknown>((frame) => {
    const text = JSON.stringify(frame);
    for (const listener of messageListeners) listener(text);
  });
  const conn = await invoke<number>("rpc_connect", { target, channel });
  if (leaving) {
    // 연결을 여는 사이 이탈이 시작됐다. 새 페이지가 이 연결을 알 길이 없으니 바로 닫는다.
    void invoke("rpc_disconnect", { conn }).catch(() => {});
    throw new Error("page is unloading");
  }

  // invoke 는 비동기라 앞의 것이 끝나기 전에 다음 것이 Rust 에서 먼저 처리될 수 있다. 요청 뒤의 request.cancel 이
  // 요청보다 먼저 닿으면 취소가 사라지므로, 보내기를 한 줄로 세운다.
  let queue: Promise<unknown> = Promise.resolve();
  const transport: Transport = {
    send(text) {
      if (closed) throw new Error("tauri transport closed");
      const frame: unknown = JSON.parse(text);
      queue = queue
        .then(() => invoke("rpc_send", { conn, frame }))
        // rpc_send 는 연결이 없을 때만 실패한다(창 · 페이지 정리로 닫혔다). 끊김으로 알린다.
        .catch(markClosed);
    },
    onMessage(listener) {
      messageListeners.add(listener);
      return () => messageListeners.delete(listener);
    },
    onClose(listener) {
      closeListeners.add(listener);
      return () => closeListeners.delete(listener);
    },
  };
  return {
    transport,
    async close() {
      if (closed) return;
      markClosed();
      await queue;
      await invoke("rpc_disconnect", { conn }).catch(() => {});
    },
  };
}
