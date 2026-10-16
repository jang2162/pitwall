// pw-rpc 이벤트 구독 → 쿼리 무효화(docs/design/02-tech-stack.md 「UI 상태」 행: 코어 이벤트 스트림 → 쿼리 캐시 무효화).
//
// 규칙 하나가 토픽 하나에 무효화할 쿼리 키(접두사)를 붙인다. 무효화된 쿼리 중 화면에 붙어 있는 것만 다시 불린다.
// ⚠ 이벤트는 프로젝트 범위만 RPC 로 나간다(crates/pw-core/src/bus.rs 의 `EventBus::publish`). Host 범위 이벤트
// (`task.progress` 등 Host 작업의 알림)는 구독해도 오지 않는다.

import type { EventMessage, RpcClient, Subscription } from "@pitwall/rpc-client";
import type { QueryClient, QueryKey } from "@tanstack/react-query";

export interface InvalidationRule {
  topic: string;
  /** 무효화할 쿼리 키(접두사). 이벤트 payload 로 좁힐 수 있다. */
  invalidate(event: EventMessage): readonly QueryKey[];
}

export interface EventInvalidation {
  /** 구독이 열리면 풀린다. 구독이 실패해도 풀린다(실패는 경고로 남긴다). */
  readonly ready: Promise<void>;
  /** 구독을 닫는다. 그 뒤에 도착한 이벤트는 무효화하지 않는다. */
  dispose(): void;
}

export function connectEventInvalidation(
  client: RpcClient,
  queryClient: QueryClient,
  projectId: string,
  rules: readonly InvalidationRule[],
): EventInvalidation {
  const topics = [...new Set(rules.map((r) => r.topic))];
  let disposed = false;
  let subscription: Subscription | undefined;

  const onEvent = (event: EventMessage) => {
    if (disposed) return;
    for (const rule of rules) {
      if (rule.topic !== event.topic) continue;
      for (const queryKey of rule.invalidate(event))
        void queryClient.invalidateQueries({ queryKey });
    }
  };

  const ready =
    topics.length === 0
      ? Promise.resolve()
      : client
          .subscribe({ projectId, topics }, onEvent)
          .then((opened) => {
            subscription = opened;
            // 구독이 열리기 전에 dispose 됐으면 연 즉시 닫는다.
            if (disposed) void opened.unsubscribe().catch(() => {});
          })
          .catch((error: unknown) => console.warn("[rpc] 이벤트 구독 실패", topics, error));

  return {
    ready,
    dispose() {
      if (disposed) return;
      disposed = true;
      void subscription?.unsubscribe().catch(() => {});
    },
  };
}
