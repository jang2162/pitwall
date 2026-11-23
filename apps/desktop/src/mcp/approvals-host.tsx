// 창의 MCP 승인(16-m5-plan M5-14): 창이 떠 있는 동안 `mcp.approvals` 를 듣고(approvals.ts), 지금 보일 요청의 다이얼로그
// (approval-dialog.tsx)를 띄우고, 새 요청마다 OS 알림을 청한다(approval-native.ts — 창이 앞에 있으면 네이티브가 아무것도 하지
// 않는다). 프로젝트 창(`projectId`)과 웰컴 · 설정 창(`null` — 프로젝트 없는 요청만)이 하나씩 둔다.

import type { McpApprovalRequest, RpcClient } from "@pitwall/rpc-client";
import { useEffect, useRef, useState } from "react";
import { useStore } from "zustand";

import { LOCAL_MACHINE } from "../checkout/checkout";
import { useCheckoutHost } from "../checkout/checkout-hosts";
import { useHost } from "../rpc/host";
import { agentLine } from "./agent-text";
import { ApprovalDialog } from "./approval-dialog";
import { type ApprovalNative, tauriApprovalNative } from "./approval-native";
import {
  answerApproval,
  approvalStore,
  type ApprovalStore,
  currentRequest,
  dismiss,
  expire,
  listenApprovals,
  reveal,
} from "./approvals";
import { type McpStrings, useMcpStrings } from "./strings";

/** 남은 시한을 다시 그리는 간격. */
export const CLOCK_MS = 1000;

/** 알림의 에이전트 · 툴 이름 글자 상한. */
const NOTICE_NAME_CHARS = 60;

/** 대기가 있는 동안 `CLOCK_MS` 마다 지금 시각(Unix 밀리초)을 주고, 시한이 지난 요청을 뺀다. */
export function useApprovalClock(store: ApprovalStore = approvalStore): number {
  const waiting = useStore(store, (s) => s.requests.length > 0);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!waiting) return;
    const tick = () => {
      const at = Date.now();
      setNow(at);
      expire(store, at);
    };
    tick();
    const timer = setInterval(tick, CLOCK_MS);
    return () => clearInterval(timer);
  }, [waiting, store]);
  return now;
}

/** OS 알림의 글: 「<에이전트>가 <툴> 승인을 기다린다」. 에이전트가 보낸 이름은 한 줄 규칙으로 걸러 짧게. */
export function noticeOf(request: McpApprovalRequest, T: McpStrings["approval"]) {
  const agent =
    request.client === null ? T.notifyAgent : agentLine(request.client, NOTICE_NAME_CHARS);
  return {
    request: request.id,
    title: T.notifyTitle,
    body: T.notifyBody(agent, agentLine(request.tool, NOTICE_NAME_CHARS)),
  };
}

/** 창의 승인. 창이 하나씩 둔다(App.tsx · layout/project-window.tsx). */
export function McpApprovals({ projectId }: { projectId: string | null }) {
  const { client } = useHost();
  if (!client) return null;
  return <WindowApprovals client={client} projectId={projectId} />;
}

/**
 * 프로젝트 창의 승인(18-m7-plan M7-18): Desktop 의 승인 허브는 **늘 창의 이 머신 연결 · 이 머신 프로젝트 id** 로 듣는다 — 활성
 * 체크아웃이 다른 머신이어도 그렇다. 다른 머신을 겨누는 이 Desktop 세션의 요청도 묶음의 로컬 멤버 id 로 온다. 활성 체크아웃의
 * 연결로 들으면 원격이 활성인 동안 이 창이 허브를 듣지 않아 요청이 「창 없음」 으로 거절된다. 원격 머신 Relay 의 허브를 듣는 것은
 * M7-20 이 더한다. 이 머신 체크아웃이 없는 묶음 창(원격만)은 듣지 않는다.
 */
export function ProjectWindowApprovals() {
  const local = useCheckoutHost(LOCAL_MACHINE);
  if (!local.client || local.projectId === undefined) return null;
  return <WindowApprovals client={local.client} projectId={local.projectId} />;
}

/** `McpApprovals` 의 몸통 — 시험은 가짜 클라이언트 · 알림 · 저장소를 넘긴다. */
export function WindowApprovals({
  client,
  projectId,
  native = tauriApprovalNative,
  store = approvalStore,
  armDelayMs,
}: {
  client: RpcClient;
  /** 프로젝트 창이면 그 프로젝트, 웰컴 · 설정 창이면 `null`. */
  projectId: string | null;
  native?: ApprovalNative;
  store?: ApprovalStore;
  /** 허용 단추를 잠그는 시간(approval-dialog.tsx 의 `ARM_DELAY_MS`). 시험만 바꾼다. */
  armDelayMs?: number;
}) {
  const T = useMcpStrings().approval;
  // 알림 글은 요청이 온 때의 언어로 — 언어가 바뀌어도 스트림을 다시 열지 않는다.
  const strings = useRef(T);
  strings.current = T;
  useEffect(
    () =>
      listenApprovals(client, projectId, store, (request) =>
        native.notify(noticeOf(request, strings.current)),
      ),
    [client, projectId, store, native],
  );
  useEffect(() => native.onReveal((id) => reveal(store, id)), [native, store]);
  const now = useApprovalClock(store);
  const state = useStore(store);
  const current = currentRequest(state);
  if (!current) return null;
  return (
    <ApprovalDialog
      key={current.id}
      request={current}
      index={state.requests.indexOf(current) + 1}
      total={state.requests.length}
      now={now}
      onAnswer={(answer) => answerApproval(client, store, current.id, answer)}
      onDismiss={() => dismiss(store, current.id)}
      armDelayMs={armDelayMs}
      epoch={state.revealed}
    />
  );
}
