// pw-rpc 이벤트 구독 → 쿼리 무효화(docs/design/02-tech-stack.md 「UI 상태」 행: 코어 이벤트 스트림 → 쿼리 캐시 무효화).
//
// 규칙 하나가 토픽 하나에 무효화할 쿼리 키(접두사)를 붙인다. 무효화된 쿼리 중 화면에 붙어 있는 것만 다시 불린다.
// 구독이 열리면 규칙의 키를 한 번 무효화한다 — 구독 전에 받아 둔 응답은 그 사이의 이벤트를 놓쳤을 수 있다.
// 구독 하나는 범위 하나만 받는다(crates/pw-rpc/src/events.rs 머리 주석). 프로젝트 이벤트와 Host 범위 이벤트(전역 설정
// 변경 등)를 둘 다 받는 창은 이것을 범위마다 한 번씩 부른다.

import type { EventMessage, RpcClient, Subscription } from "@pitwall/rpc-client";
import type { QueryClient, QueryKey } from "@tanstack/react-query";

export interface InvalidationRule {
  topic: string;
  /** 이 토픽이 무효화하는 쿼리 키(접두사). 구독이 열릴 때도 이것을 무효화한다. */
  keys: readonly QueryKey[];
  /** 이벤트 payload 로 무효화할 키를 좁힌다. 없으면 `keys` 를 쓴다. */
  narrow?(event: EventMessage): readonly QueryKey[];
}

export interface EventInvalidation {
  /** 구독이 열리면 풀린다. 구독이 실패해도 풀린다(실패는 경고로 남긴다). */
  readonly ready: Promise<void>;
  /** 구독을 닫는다. 그 뒤에 도착한 이벤트는 무효화하지 않는다. */
  dispose(): void;
}

/** `projectId` 가 `null` 이면 Host 범위를 구독한다. */
export function connectEventInvalidation(
  client: RpcClient,
  queryClient: QueryClient,
  projectId: string | null,
  rules: readonly InvalidationRule[],
): EventInvalidation {
  const topics = [...new Set(rules.map((r) => r.topic))];
  let disposed = false;
  let subscription: Subscription | undefined;

  const invalidate = (keys: readonly QueryKey[]) => {
    for (const queryKey of keys) void queryClient.invalidateQueries({ queryKey });
  };

  const onEvent = (event: EventMessage) => {
    if (disposed) return;
    for (const rule of rules) {
      if (rule.topic !== event.topic) continue;
      invalidate(rule.narrow?.(event) ?? rule.keys);
    }
  };

  const ready =
    topics.length === 0
      ? Promise.resolve()
      : client
          .subscribe(projectId === null ? { topics } : { projectId, topics }, onEvent)
          .then((opened) => {
            subscription = opened;
            // 구독이 열리기 전에 dispose 됐으면 연 즉시 닫는다.
            if (disposed) {
              void opened.unsubscribe().catch(() => {});
              return;
            }
            for (const rule of rules) invalidate(rule.keys);
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
