// 이 창에 온 MCP 승인 요청(16-m5-plan M5-14, Host 쪽은 crates/pw-mcp/src/approval.rs). 창이 떠 있는 동안 `mcp.approvals` 스트림을
// 하나 열어 두고, 승인 다이얼로그(approval-dialog.tsx)와 「에이전트 활동」 의 「승인 대기」 칸(pending-approvals.tsx)이 이 저장소
// 하나를 읽는다 — 두 벌이 되면 한쪽에서 답한 요청이 다른 쪽에 남는다(M5-13 리뷰 R4).
//
// - 프로젝트 창은 자기 프로젝트로(`{ projectId }`), 웰컴 · 설정 창은 프로젝트 없이(`{}`) 듣는다. 프로젝트 창도 프로젝트 없는
//   요청(`notThisProject`)을 받는다 — Host 가 가장 최근에 앞에 있던 창 하나에 보낸다(결정 M5-D29).
// - 다이얼로그는 한 번에 하나를 보인다: 목록에서 다시 연 요청(`front`), 없으면 닫지(Esc) 않은 것 가운데 id 가 가장 작은 것.
//   Esc 로 닫은 요청은 목록에 남고 시한까지 Host 에서 기다린다.
// - 답하면 저장소에서 곧바로 빼고 `mcp.approval.answer` 를 보낸다. 그 사이 Host 가 거뒀으면(시한 · 끊김) not found 이고 경고만
//   남긴다. Host 의 닫힘(`closed`)과 시한 끝(`expiresAt`, 이 창의 시계)은 둘 다 그 요청을 뺀다 — 시계가 Host 보다 빠르거나 늦어도
//   다이얼로그가 시한 뒤에 남아 있지 않다.
// - 대기는 Host 가 세션당 8개로 묶는다(`MAX_PENDING_PER_SESSION`). 창 하나에는 여러 세션의 요청이 함께 올 수 있어 목록은 앞
//   몇 줄만 그린다(pending-approvals.tsx 의 `LISTED`).

import type { McpApprovalAnswer, McpApprovalRequest, RpcClient } from "@pitwall/rpc-client";
import { createStore, type StoreApi } from "zustand/vanilla";

const LOG = "[mcp]";

export interface ApprovalsState {
  /** 이 창에 보이는 대기(id 순). */
  readonly requests: readonly McpApprovalRequest[];
  /** Esc 로 닫은 요청. 목록에는 남고 다이얼로그는 저절로 다시 뜨지 않는다. */
  readonly dismissed: ReadonlySet<number>;
  /** 목록에서 눌러 다시 연 요청 — 다이얼로그가 이것을 먼저 보인다. */
  readonly front: number | undefined;
}

export type ApprovalStore = StoreApi<ApprovalsState>;

const EMPTY: ApprovalsState = { requests: [], dismissed: new Set(), front: undefined };

export function createApprovalStore(): ApprovalStore {
  return createStore<ApprovalsState>(() => EMPTY);
}

/** 이 창(웹뷰 하나)의 저장소. 시험은 `resetApprovals` 로 비운다. */
export const approvalStore: ApprovalStore = createApprovalStore();

export function resetApprovals(store: ApprovalStore = approvalStore): void {
  store.setState(EMPTY, true);
}

/** 새 요청(같은 id 가 다시 오면 — 듣기를 다시 열었을 때 — 바꿔 넣는다). */
export function opened(store: ApprovalStore, request: McpApprovalRequest): void {
  store.setState((s) => ({
    requests: [...s.requests.filter((r) => r.id !== request.id), request].sort(
      (a, b) => a.id - b.id,
    ),
  }));
}

/** 그 요청이 끝났다(Host 의 닫힘 · 답함 · 시한). */
export function closed(store: ApprovalStore, id: number): void {
  store.setState((s) => {
    if (!s.requests.some((r) => r.id === id)) return s;
    const dismissed = new Set(s.dismissed);
    dismissed.delete(id);
    return {
      requests: s.requests.filter((r) => r.id !== id),
      dismissed,
      front: s.front === id ? undefined : s.front,
    };
  });
}

/** Esc — 다이얼로그만 닫는다. */
export function dismiss(store: ApprovalStore, id: number): void {
  store.setState((s) => ({
    dismissed: new Set(s.dismissed).add(id),
    front: s.front === id ? undefined : s.front,
  }));
}

/** 목록 · OS 알림에서 그 요청을 다시 연다. 없는 요청이면 아무것도 하지 않는다. */
export function reveal(store: ApprovalStore, id: number): void {
  store.setState((s) => {
    if (!s.requests.some((r) => r.id === id)) return s;
    const dismissed = new Set(s.dismissed);
    dismissed.delete(id);
    return { dismissed, front: id };
  });
}

/** `now`(Unix 밀리초)에 시한이 끝난 요청을 뺀다. */
export function expire(store: ApprovalStore, now: number): void {
  for (const r of store.getState().requests) if (r.expiresAt <= now) closed(store, r.id);
}

/** 다이얼로그가 보일 요청. */
export function currentRequest(state: ApprovalsState): McpApprovalRequest | undefined {
  const front = state.requests.find((r) => r.id === state.front);
  return front ?? state.requests.find((r) => !state.dismissed.has(r.id));
}

/** 답한다 — 저장소에서 곧바로 빼고 Host 에 보낸다. */
export function answerApproval(
  client: RpcClient,
  store: ApprovalStore,
  id: number,
  answer: McpApprovalAnswer,
): void {
  closed(store, id);
  client.call("mcp.approval.answer", { id, answer }).catch((error: unknown) => {
    // 그사이 Host 가 거뒀으면(시한 · 세션 끊김 · 호출 취소) not found 다.
    console.warn(LOG, "could not answer the approval request", id, error);
  });
}

/**
 * `mcp.approvals` 를 열어 두고 저장소를 채운다. `onOpen` 은 새로 온 요청마다(OS 알림). 스트림이 끊기면 경고를 남기고 저장소를
 * 비운다 — Host 는 듣는 창이 없는 것으로 보고 그 요청들을 거둔다(답할 길이 없는 다이얼로그를 남기지 않는다). 돌려준 함수가 닫는다.
 */
export function listenApprovals(
  client: RpcClient,
  projectId: string | null,
  store: ApprovalStore,
  onOpen?: (request: McpApprovalRequest) => void,
): () => void {
  const abort = new AbortController();
  void (async () => {
    try {
      const params = projectId === null ? {} : { projectId };
      for await (const event of client.stream("mcp.approvals", params, { signal: abort.signal })) {
        if (abort.signal.aborted) return;
        if (event.kind === "closed") {
          closed(store, event.id);
          continue;
        }
        const known = store.getState().requests.some((r) => r.id === event.request.id);
        opened(store, event.request);
        if (!known) onOpen?.(event.request);
      }
    } catch (error) {
      if (!abort.signal.aborted)
        console.warn(LOG, "stopped listening for approval requests", error);
    } finally {
      if (!abort.signal.aborted) resetApprovals(store);
    }
  })();
  return () => {
    abort.abort();
    resetApprovals(store);
  };
}
