// 「에이전트 활동」 패널 맨 위의 「승인 대기」 칸(16-m5-plan M5-13). 지금은 자리만 있다 — 승인 다이얼로그 행(M5-14)이 이 파일의
// 몸통을 채운다: `mcp.approvals` 스트림의 대기 요청을 줄마다(에이전트 · 툴 · 등급 · 남은 시한) 보이고, 누르면 그 다이얼로그를
// 다시 연다(`Esc` 로 닫은 요청이 여기 남는다).
//
// 칸의 꼴: 바깥 `section[data-testid=mcp-approvals]` 와 제목(`aria-labelledby`)은 그대로 두고, 그 아래 내용만 바꾼다. 대기가
// 없을 때는 `[data-testid=mcp-approvals-empty]` 한 줄이다. 패널(agent-activity.tsx)은 이 컴포넌트에 프로젝트 id 만 넘긴다.

import { useId } from "react";

import { useMcpStrings } from "./strings";

export function PendingApprovals(_props: { projectId: string }) {
  const T = useMcpStrings().activity;
  const title = useId();
  return (
    <section
      data-testid="mcp-approvals"
      aria-labelledby={title}
      className="flex-none border-b border-ui-border px-2 py-1"
    >
      <h3 id={title} className="text-[11px] font-semibold text-ui-muted-foreground uppercase">
        {T.approvalsTitle}
      </h3>
      <p data-testid="mcp-approvals-empty" className="text-[12px] text-ui-muted-foreground">
        {T.approvalsEmpty}
      </p>
    </section>
  );
}
