// 「에이전트 활동」 패널 맨 위의 「승인 대기」 칸(16-m5-plan M5-13 이 자리를 두고 M5-14 가 채웠다): 이 창에 온 승인 요청을 줄마다
// (에이전트 · 툴 · 다른 머신이면 그 머신 · 등급 · 남은 시한) 보이고, 줄을 누르면 그 다이얼로그를 다시 연다 — `Esc` 로 닫은 요청이 여기 남는다.
//
// - 대기 목록은 다이얼로그와 같은 창 단위 저장소(approvals.ts 의 `approvalStore`)를 읽는다. 이 칸이 스트림을 따로 열지 않는다 —
//   두 벌이 되면 한쪽에서 답한 요청이 다른 쪽에 남는다(M5-13 리뷰 R4). 스트림은 창의 `McpApprovals`(approvals-host.tsx)가 연다.
// - 에이전트가 보낸 글(클라이언트 이름)은 다이얼로그와 같은 넓은 규칙(agent-text.ts — DB 그리드의 `INVISIBLE`)으로 거른다(R6) —
//   `claude + U+200B + code` 같은 흉내가 보인다.
// - 앞 `LISTED` 줄만 그리고 나머지는 「그리고 n개 더」 — Host 가 세션당 8개로 묶지만 여러 세션의 요청이 한 창에 모일 수 있다.
//
// 칸의 꼴: 바깥 `section[data-testid=mcp-approvals]` 와 제목(`aria-labelledby`)은 M5-13 의 것 그대로다. 대기가 없을 때는
// `[data-testid=mcp-approvals-empty]` 한 줄이다. 패널(agent-activity.tsx)은 이 컴포넌트에 프로젝트 id 만 넘긴다.

import { useId } from "react";
import { useStore } from "zustand";

import { agentLine } from "./agent-text";
import { clock, MachineName } from "./approval-dialog";
import { useApprovalClock } from "./approvals-host";
import { approvalStore, type ApprovalStore, reveal } from "./approvals";
import { useMcpStrings } from "./strings";

/** 목록에 그리는 줄 수. */
export const LISTED = 20;

export function PendingApprovals({
  store = approvalStore,
}: {
  projectId: string;
  store?: ApprovalStore;
}) {
  const strings = useMcpStrings();
  const T = strings.activity;
  const A = strings.approval;
  const title = useId();
  const requests = useStore(store, (s) => s.requests);
  const now = useApprovalClock(store);
  const shown = requests.slice(0, LISTED);
  const more = requests.length - shown.length;
  return (
    <section
      data-testid="mcp-approvals"
      aria-labelledby={title}
      className="flex-none border-b border-ui-border px-2 py-1"
    >
      <h3 id={title} className="text-[11px] font-semibold text-ui-muted-foreground uppercase">
        {T.approvalsTitle}
      </h3>
      {requests.length === 0 ? (
        <p data-testid="mcp-approvals-empty" className="text-[12px] text-ui-muted-foreground">
          {T.approvalsEmpty}
        </p>
      ) : (
        <ul className="flex flex-col">
          {shown.map((request) => {
            const agent = request.client === null ? A.unknownAgent : agentLine(request.client, 60);
            const tool = agentLine(request.tool, 60);
            return (
              <li key={request.id}>
                <button
                  type="button"
                  data-testid="mcp-approval-row"
                  data-request={request.id}
                  aria-label={T.approvalsOpen(agent, tool)}
                  onClick={() => reveal(store, request.id)}
                  className="flex w-full items-baseline gap-2 rounded-sm px-1 py-0.5 text-left text-[12px] hover:bg-ui-hover"
                >
                  <span className="min-w-0 flex-1 truncate" title={agent}>
                    {agent}
                  </span>
                  <span className="min-w-0 truncate font-mono">{tool}</span>
                  {request.machine !== null && (
                    <span
                      data-testid="mcp-approval-row-machine"
                      className="max-w-[120px] truncate rounded-sm border border-ui-border px-1 text-[11px] text-ui-muted-foreground"
                    >
                      <MachineName machine={request.machine} />
                    </span>
                  )}
                  <span
                    className={request.tier === "dangerous" ? "font-semibold text-ui-accent" : ""}
                  >
                    {A.tier[request.tier]}
                  </span>
                  <span className="text-ui-muted-foreground tabular-nums">
                    {clock(request.expiresAt - now)}
                  </span>
                </button>
              </li>
            );
          })}
          {more > 0 && (
            <li
              data-testid="mcp-approvals-more"
              className="px-1 text-[12px] text-ui-muted-foreground"
            >
              {T.approvalsMore(more)}
            </li>
          )}
        </ul>
      )}
    </section>
  );
}
