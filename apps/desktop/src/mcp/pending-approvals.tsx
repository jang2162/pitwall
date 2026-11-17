// 「에이전트 활동」 패널 맨 위의 「승인 대기」 칸(16-m5-plan M5-13). 지금은 자리만 있다 — 승인 다이얼로그 행(M5-14)이 이 파일의
// 몸통을 채운다: `mcp.approvals` 스트림의 대기 요청을 줄마다(에이전트 · 툴 · 등급 · 남은 시한) 보이고, 누르면 그 다이얼로그를
// 다시 연다(`Esc` 로 닫은 요청이 여기 남는다).
//
// 칸의 꼴: 바깥 `section[data-testid=mcp-approvals]` 와 제목(`aria-labelledby`)은 그대로 두고, 그 아래 내용만 바꾼다. 대기가
// 없을 때는 `[data-testid=mcp-approvals-empty]` 한 줄이다. 패널(agent-activity.tsx)은 이 컴포넌트에 프로젝트 id 만 넘긴다.
//
// M5-14 에 남기는 것(M5-13 리뷰 R4 · R6):
// - 이 칸은 툴 윈도우가 있을 때만 그려진다. 대기 목록은 이 컴포넌트가 `mcp.approvals` 스트림을 따로 열어 쥐지 말고, 다이얼로그가
//   쓰는 창 단위 저장소를 읽는다 — 두 벌이 되면 한쪽에서 답한 요청이 다른 쪽에 남는다.
// - 에이전트가 보낸 글(클라이언트 이름 · 경로)을 사람이 보고 허용하는 자리라, 이 패널의 `displayText`(제어 문자 · 양방향 제어 문자)
//   보다 넓은 DB 그리드의 규칙(`apps/desktop/src/db/grid/values.ts` 의 `INVISIBLE` — 0폭 문자 · 결합자 · BOM 까지)을 쓰는 편이
//   안전하다. `claude\u200bcode` 같은 흉내를 보이게 한다.

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
