// LSP 기능 행의 `services.subscribe`(src/lsp/lsp-sessions.ts): 창의 프로젝트 범위 이벤트를 듣는다. 창의 Host 연결이 아직 없으면
// 던지지 않고 기다렸다가 연결이 생기면 붙인다 — 기능 행은 뷰 플러그인 생성자에서 부르므로 던지면 그 뷰 구성이 깨진다(재리뷰 N-4).
// 구독은 비동기로 붙으므로 붙기 전에 해제하면 붙자마자 뗀다.

import type { EventMessage, SubscribeParams, Subscription } from "@pitwall/rpc-client";

export interface SubscribeClient {
  subscribe(
    params: SubscribeParams,
    listener: (event: EventMessage) => void,
  ): Promise<Subscription>;
}

export interface ProjectSubscriptions {
  /** 창의 Host 연결이 생겼다. 기다리던 구독을 붙인다. `undefined` 면 아무것도 하지 않는다. */
  connect(client: SubscribeClient | undefined): void;
  /** 이 창의 프로젝트의 이벤트 `topics` 를 듣는다. 해제 함수를 돌려준다. */
  subscribe(topics: readonly string[], listener: (event: EventMessage) => void): () => void;
}

function subscribeOn(
  client: SubscribeClient,
  projectId: string,
  topics: readonly string[],
  listener: (event: EventMessage) => void,
): () => void {
  let closed = false;
  let subscription: Subscription | undefined;
  client
    .subscribe({ projectId, topics: [...topics] }, (event) => {
      if (!closed) listener(event);
    })
    .then((s) => {
      subscription = s;
      if (closed) void s.unsubscribe().catch(() => {});
    })
    .catch((e: unknown) => console.warn("[lsp] subscription failed", topics, e));
  return () => {
    closed = true;
    void subscription?.unsubscribe().catch(() => {});
  };
}

export function projectSubscriptions(projectId: string): ProjectSubscriptions {
  let client: SubscribeClient | undefined;
  const waiting = new Set<(client: SubscribeClient) => void>();
  return {
    connect(next) {
      client = next;
      if (!next) return;
      const attach = [...waiting];
      waiting.clear();
      for (const a of attach) a(next);
    },
    subscribe(topics, listener) {
      let closed = false;
      let detach: (() => void) | undefined;
      const attach = (c: SubscribeClient) => {
        if (!closed) detach = subscribeOn(c, projectId, topics, listener);
      };
      if (client) attach(client);
      else waiting.add(attach);
      return () => {
        closed = true;
        waiting.delete(attach);
        detach?.();
      };
    },
  };
}
