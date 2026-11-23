// 창의 MCP 승인(16-m5-plan M5-14): 창이 떠 있는 동안 `mcp.approvals` 를 듣고(approvals.ts), 지금 보일 요청의 다이얼로그
// (approval-dialog.tsx)를 띄우고, 새 요청마다 OS 알림을 청한다(approval-native.ts — 창이 앞에 있으면 네이티브가 아무것도 하지
// 않는다). 프로젝트 창(`projectId`)과 웰컴 · 설정 창(`null` — 프로젝트 없는 요청만)이 하나씩 둔다.

import type { McpApprovalRequest, RpcClient } from "@pitwall/rpc-client";
import { useEffect, useMemo, useRef, useState } from "react";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import { LOCAL_MACHINE } from "../checkout/checkout";
import { isBundleRowId, useCheckoutHost, useCheckoutHosts } from "../checkout/checkout-hosts";
import { useHost } from "../rpc/host";
import { agentLine } from "./agent-text";
import { ApprovalDialog } from "./approval-dialog";
import { type ApprovalNative, tauriApprovalNative } from "./approval-native";
import {
  answerApproval,
  approvalStore,
  type ApprovalStore,
  approvalStoreFor,
  currentRequest,
  dismiss,
  expire,
  listenApprovals,
  listening,
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
 * 프로젝트 창의 승인(18-m7-plan M7-18 · M7-20).
 *
 * - **이 Desktop 의 허브**는 늘 창의 이 머신 연결 · 이 머신 프로젝트 id 로 듣는다 — 활성 체크아웃이 다른 머신이어도 그렇다. 다른 머신을
 *   겨누는 이 Desktop 세션의 요청도 묶음의 로컬 멤버 id 로 온다. 이 머신 체크아웃이 없는 묶음 창(원격만)은 창 연결의 묶음 id 로
 *   듣는다(M7-20 — 그 묶음의 요청은 묶음 id 로 온다).
 * - **다른 머신 Relay 의 허브**(그 머신에서 도는 에이전트의 요청 — M7-20 ③)는 그 머신 연결이 열려 있는 동안(활성 체크아웃이거나 탭이
 *   열린 머신 — 연결은 쓰는 머신만 연다, 08 「창과 활성 체크아웃」) 그 연결 · 그 머신의 프로젝트 id 로 듣는다. 이것만으로 링크를 열지
 *   않는다 — 붙은 owner Desktop 이 없는 동안 그 머신의 에이전트는 `write` 이상을 거절받는다(결정 M7-D10 ④). 요청은 머신마다의
 *   저장소(`approvalStoreFor`)에 들고, 다이얼로그는 한 번에 하나 — 이 Desktop 의 요청이 먼저다.
 */
export function ProjectWindowApprovals() {
  const local = useCheckoutHost(LOCAL_MACHINE);
  const hosts = useCheckoutHosts();
  const remote = useStore(hosts?.store ?? NO_HOSTS, (s) => s.remote);
  const members = useStore(hosts?.store ?? NO_HOSTS, (s) => s.directory.members);
  // 이 머신 프로젝트 창이면 그 프로젝트 id, 원격만 묶음 창이면 창 연결의 묶음 id 로 듣는다 — 그 묶음의 다른 머신을 겨눈 이 Desktop
  // 세션의 요청은 묶음 id 로 온다(18-m7-plan M7-20 · M7-11b 판정). 묶음 id 는 프로젝트 id 와 겹치지 않는다(`b-`).
  const ownId = "projectId" in local.target ? local.target.projectId : undefined;
  const hearAs =
    local.projectId ?? (ownId !== undefined && isBundleRowId(ownId) ? ownId : undefined);
  const localClient = hearAs === undefined ? undefined : local.client;
  const listening = useMemo(() => {
    const out: { machine: string; client: RpcClient; projectId: string; store: ApprovalStore }[] =
      [];
    for (const [machine, r] of Object.entries(remote)) {
      const projectId = members.get(machine);
      if (machine === LOCAL_MACHINE || !r.client || projectId === undefined) continue;
      out.push({ machine, client: r.client, projectId, store: approvalStoreFor(machine) });
    }
    return out;
  }, [remote, members]);
  // 앞의 저장소에 보일 요청이 있으면 뒤의 다이얼로그는 기다린다 — 둘이 한꺼번에 뜨지 않게. 이 Desktop 의 요청이 먼저다.
  const before = useMemo(() => {
    const order = [...(localClient ? [approvalStore] : []), ...listening.map((l) => l.store)];
    return listening.map((l) => order.slice(0, order.indexOf(l.store)));
  }, [localClient, listening]);
  return (
    <>
      {localClient && hearAs !== undefined && (
        <WindowApprovals client={localClient} projectId={hearAs} />
      )}
      {listening.map((l, i) => (
        <WindowApprovals
          key={l.machine}
          client={l.client}
          projectId={l.projectId}
          store={l.store}
          origin={l.machine}
          yieldTo={before[i]}
        />
      ))}
    </>
  );
}

const NO_HOSTS = createStore<{
  remote: Readonly<Record<string, { client?: RpcClient }>>;
  directory: { members: ReadonlyMap<string, string> };
}>()(() => ({ remote: {}, directory: { members: new Map() } }));

const NONE: readonly ApprovalStore[] = [];

/** 저장소들 가운데 다이얼로그에 보일 요청이 있는 것이 있는가. */
function useWaiting(stores: readonly ApprovalStore[]): boolean {
  const [waiting, setWaiting] = useState(false);
  useEffect(() => {
    const check = () => setWaiting(stores.some((s) => currentRequest(s.getState()) !== undefined));
    check();
    const offs = stores.map((s) => s.subscribe(check));
    return () => offs.forEach((off) => off());
  }, [stores]);
  return waiting;
}

/** `McpApprovals` 의 몸통 — 시험은 가짜 클라이언트 · 알림 · 저장소를 넘긴다. */
export function WindowApprovals({
  client,
  projectId,
  native = tauriApprovalNative,
  store = approvalStore,
  armDelayMs,
  origin,
  yieldTo = NONE,
}: {
  client: RpcClient;
  /** 프로젝트 창이면 그 프로젝트, 웰컴 · 설정 창이면 `null`. */
  projectId: string | null;
  native?: ApprovalNative;
  store?: ApprovalStore;
  /** 허용 단추를 잠그는 시간(approval-dialog.tsx 의 `ARM_DELAY_MS`). 시험만 바꾼다. */
  armDelayMs?: number;
  /** 요청이 온 머신 — 그 머신 Relay 의 허브를 들을 때(다이얼로그의 「<머신> 의 에이전트」). 이 Desktop 의 허브면 없다. */
  origin?: string;
  /** 이 저장소들에 보일 요청이 있는 동안 다이얼로그를 띄우지 않는다(한 번에 하나). */
  yieldTo?: readonly ApprovalStore[];
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
  useEffect(() => (origin === undefined ? undefined : listening(origin, store)), [origin, store]);
  const now = useApprovalClock(store);
  const state = useStore(store);
  const waiting = useWaiting(yieldTo);
  const current = currentRequest(state);
  if (!current || waiting) return null;
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
      origin={origin}
    />
  );
}
