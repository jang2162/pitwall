// 사슬(`before` · 복합 구성 — 17-m6-plan M6-21, 규칙은 crates/pw-run/src/runs/chain.rs)의 진행을 보인다. 각 단계의 실행은
// `run.started` 로 탭이 생기므로, 여기서는 사슬이 **어느 단계에서 왜 멈췄는지**만 알린다(프로젝트 이벤트 `run.chain`). 사람이 띄운
// 사슬이든 에이전트가 띄운 사슬이든 같다 — 남은 단계가 돌지 않았다는 것을 사람이 알아야 한다.

import type { EventMessage, RunChainEvent } from "@pitwall/rpc-client";
import { useEffect, useRef } from "react";

import { useNotify } from "../notifications/notifications";
import type { RunClient } from "./run-tabs";
import { useRunStrings } from "./strings";

/** `pw_run::runs::chain::TOPIC_CHAIN`. */
export const TOPIC_CHAIN = "run.chain";

/** 실패 · 멈춤으로 끝난 사슬의 알림 글. 진행 중 · 다 띄운 사슬은 `undefined`. */
export function chainNotice(
  event: RunChainEvent,
  T: ReturnType<typeof useRunStrings>["chain"],
): { level: "error" | "info"; title: string } | undefined {
  const step = event.step ?? event.config;
  if (event.state === "failed")
    return { level: "error", title: T.failed(event.config, step, event.message ?? "") };
  if (event.state === "stopped") return { level: "info", title: T.stopped(event.config, step) };
  return undefined;
}

/** 이 창의 프로젝트 사슬을 듣고, 멈춘 사슬을 알린다. */
export function RunChainNotices({ client, projectId }: { client: RunClient; projectId: string }) {
  const T = useRunStrings().chain;
  const notify = useNotify();
  // 글 · 알림 함수가 그릴 때마다 바뀌어도 다시 구독하지 않는다.
  const latest = useRef({ T, notify });
  latest.current = { T, notify };
  useEffect(() => {
    let closed = false;
    let unsubscribe: (() => Promise<void>) | undefined;
    const onEvent = (event: EventMessage) => {
      if (closed || event.topic !== TOPIC_CHAIN) return;
      const notice = chainNotice(event.payload as unknown as RunChainEvent, latest.current.T);
      if (notice) latest.current.notify(notice);
    };
    void client
      .subscribe({ projectId, topics: [TOPIC_CHAIN] }, onEvent)
      .then((s) => {
        unsubscribe = () => s.unsubscribe();
        if (closed) void s.unsubscribe().catch(() => {});
      })
      .catch((e: unknown) => console.warn("[run] could not follow chain events", e));
    return () => {
      closed = true;
      void unsubscribe?.().catch(() => {});
    };
  }, [client, projectId]);
  return null;
}
