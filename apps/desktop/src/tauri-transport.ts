// `@pitwall/rpc-client` 의 Transport 를 Tauri IPC 로 구현한다. Rust 쪽은 src-tauri/src/ipc.rs.
//
// - rpc_page() 가 이 문서의 페이지 세대를 준다. 문서가 뜰 때 한 번 받아 둔다.
// - rpc_connect({ target, channel, page }) 가 Host 연결을 열고 연결 id 를 돌려준다. Host 가 보내는 메시지는 channel 로
//   온다. 연결이 닫히면 마지막에 CLOSED_FRAME 이 온다.
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

/** Rust 의 `ipc::CLOSED_FRAME`. 연결이 닫혔다는 채널의 마지막 메시지다(JSON-RPC 메시지가 아니다). */
const CLOSED_FRAME_KEY = "pitwallTransport";

function isClosedFrame(frame: unknown): boolean {
  return (
    typeof frame === "object" &&
    frame !== null &&
    (frame as Record<string, unknown>)[CLOSED_FRAME_KEY] === "closed"
  );
}

// 이 문서의 페이지 세대. 연결할 때마다 받지 않고 문서가 뜰 때 한 번 받는다 — 리로드 직전에 받으면 Rust 가 이미
// 새 세대로 바꾼 뒤에 처리돼 옛 문서가 새 세대를 받을 수 있다. 문서가 뜨는 시점에는 커밋이 끝나 늘 이 문서의 세대다
// (src-tauri/src/ipc.rs 모듈 주석). 옛 문서가 커밋 직전에 보낸 연결은 이 값이 낡아 Rust 가 거절한다.
const page: Promise<number> = invoke<number>("rpc_page");
// 웹뷰 밖(시험 등)에서는 거부된다. 처리하지 않은 거부로 남지 않게 하고, 실패는 connectHost 가 받는다.
page.catch(() => {});

export async function connectHost(target: HostTarget): Promise<TauriConnection> {
  const messageListeners = new Set<(text: string) => void>();
  const closeListeners = new Set<() => void>();
  let closed = false;
  const markClosed = () => {
    if (closed) return;
    closed = true;
    for (const listener of closeListeners) listener();
  };

  const channel = new Channel<unknown>((frame) => {
    if (isClosedFrame(frame)) {
      markClosed();
      return;
    }
    const text = JSON.stringify(frame);
    for (const listener of messageListeners) listener(text);
  });
  const conn = await invoke<number>("rpc_connect", { target, channel, page: await page });

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
