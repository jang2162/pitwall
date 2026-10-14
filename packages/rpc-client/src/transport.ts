/**
 * 클라이언트가 쓰는 transport. 텍스트 프레임(JSON-RPC 메시지 하나)을 보내고 받기만 한다.
 *
 * Tauri IPC · WebSocket 은 이 인터페이스를 구현해 꽂는다. Rust 쪽 `Session::handle_text` 와 같은 단위다.
 */
export interface Transport {
  /** 보내지 못하면(연결이 끊김 등) 던진다. 클라이언트는 그 호출만 오류로 끝낸다. */
  send(text: string): void;
  /** 받은 프레임마다 `listener` 를 부른다. 돌려준 함수를 부르면 해제된다. */
  onMessage(listener: (text: string) => void): () => void;
  /**
   * 연결이 끊기면 한 번 `listener` 를 부른다. 클라이언트는 대기 중인 요청 · 스트림 · 구독을 모두 오류로 끝낸다.
   * 끊김을 알 수 없는 transport 만 생략한다 — 생략하면 끊긴 뒤의 대기 항목은 `close()` 전까지 남는다.
   */
  onClose?(listener: () => void): () => void;
}

/** 테스트용 transport. `close()` 로 양쪽 모두에 끊김을 알린다. 닫힌 뒤 `send` 는 던진다. */
export interface MemoryTransport extends Transport {
  onClose(listener: () => void): () => void;
  close(): void;
}

/**
 * 서로 연결된 in-memory transport 두 개. 한쪽이 보낸 프레임은 다른 쪽에 마이크로태스크로 도착한다 —
 * 실제 transport 처럼 `send` 안에서 동기적으로 되돌아오지 않는다. 테스트용.
 */
export function createMemoryTransportPair(): [MemoryTransport, MemoryTransport] {
  const listeners: [Set<(text: string) => void>, Set<(text: string) => void>] = [
    new Set(),
    new Set(),
  ];
  const closeListeners = new Set<() => void>();
  let closed = false;
  const close = () => {
    if (closed) return;
    closed = true;
    for (const listener of closeListeners) listener();
  };
  const side = (self: 0 | 1): MemoryTransport => ({
    send(text) {
      if (closed) throw new Error("memory transport closed");
      const peer = listeners[self === 0 ? 1 : 0];
      queueMicrotask(() => {
        for (const listener of peer) listener(text);
      });
    },
    onMessage(listener) {
      listeners[self].add(listener);
      return () => listeners[self].delete(listener);
    },
    onClose(listener) {
      closeListeners.add(listener);
      return () => closeListeners.delete(listener);
    },
    close,
  });
  return [side(0), side(1)];
}
