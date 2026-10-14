/**
 * 클라이언트가 쓰는 transport. 텍스트 프레임(JSON-RPC 메시지 하나)을 보내고 받기만 한다.
 *
 * Tauri IPC · WebSocket 은 이 인터페이스를 구현해 꽂는다. Rust 쪽 `Session::handle_text` 와 같은 단위다.
 */
export interface Transport {
  send(text: string): void;
  /** 받은 프레임마다 `listener` 를 부른다. 돌려준 함수를 부르면 해제된다. */
  onMessage(listener: (text: string) => void): () => void;
}

/**
 * 서로 연결된 in-memory transport 두 개. 한쪽이 보낸 프레임은 다른 쪽에 마이크로태스크로 도착한다 —
 * 실제 transport 처럼 `send` 안에서 동기적으로 되돌아오지 않는다. 테스트용.
 */
export function createMemoryTransportPair(): [Transport, Transport] {
  const listeners: [Set<(text: string) => void>, Set<(text: string) => void>] = [
    new Set(),
    new Set(),
  ];
  const side = (self: 0 | 1): Transport => ({
    send(text) {
      const peer = listeners[self === 0 ? 1 : 0];
      queueMicrotask(() => {
        for (const listener of peer) listener(text);
      });
    },
    onMessage(listener) {
      listeners[self].add(listener);
      return () => listeners[self].delete(listener);
    },
  });
  return [side(0), side(1)];
}
