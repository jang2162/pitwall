// 상태바 `mcp` 항목과 웰컴 창의 연결 수(16-m5-plan M5-10 ⑤, 10-ui.md 「S4. 프로젝트 창」 그림의 「MCP 2」 · 「S1. 웰컴」 의
// 「MCP: 연결 n」).
//
// - 상태바: 이 프로젝트를 세션 프로젝트로 가진 MCP 세션 수. 0 이면 항목을 그리지 않는다. 누르면 「에이전트 활동」 툴 윈도우를 연다
//   (`toolWindow.agent` 액션 — M5-13, agent-activity.tsx). M5-13 전에는 세션 목록 팝업이었다 — 그 목록은 패널의 「연결된 에이전트」
//   칸으로 옮겼다.
// - 웰컴 창: 이 앱에 열린 세션 전부의 수.
// - 수는 `mcp.sessions`(sessions.ts)에서 센다. 세션이 열리고 닫히면 `mcp.sessionsChanged` 로 다시 읽는다.

import { Bot } from "lucide-react";
import { useEffect } from "react";
import { createStore, useStore } from "zustand";

import { useKeymap } from "../keymap/keymap-provider";
import { STATUS_SLOTS, useStatusBarItem } from "../status-bar/status-bar";
import { sessionsOf, useMcpSessions } from "./sessions";
import { useMcpStrings } from "./strings";

/** 이 창의 프로젝트. 항목 컴포넌트는 props 를 받지 않는다(status-bar.tsx 머리 주석) — 창마다 웹뷰가 따로라 모듈 상태로 둔다. */
const mcpState = createStore<{ projectId: string | undefined }>()(() => ({
  projectId: undefined,
}));

/** 이 창의 프로젝트(프로젝트 창이 아니면 `undefined`). 「에이전트 활동」 툴 윈도우도 이것으로 프로젝트를 안다. */
export function useMcpProjectId(): string | undefined {
  return useStore(mcpState, (s) => s.projectId);
}

function McpStatusFor({ projectId }: { projectId: string }) {
  const T = useMcpStrings();
  const mine = sessionsOf(useMcpSessions(), projectId);
  const { registry } = useKeymap();
  if (mine.length === 0) return null;
  return (
    <button
      type="button"
      data-testid="status-mcp"
      title={T.status.hint(mine.length)}
      onClick={() => void registry.run("toolWindow.agent")}
      className="flex items-center gap-1 rounded-sm px-1 hover:bg-ui-hover"
    >
      <Bot size={11} aria-hidden />
      {T.status.label(mine.length)}
    </button>
  );
}

/** 상태바 항목: 이 프로젝트의 MCP 세션 수. */
export function McpStatusItem() {
  const projectId = useMcpProjectId();
  return projectId === undefined ? null : <McpStatusFor projectId={projectId} />;
}

/** 이 창의 프로젝트로 MCP 항목을 둔다. `StatusBarProvider` 안에 둔다. */
export function McpStatus({ projectId }: { projectId: string }) {
  useEffect(() => {
    mcpState.setState({ projectId });
    return () => mcpState.setState({ projectId: undefined });
  }, [projectId]);
  useStatusBarItem({ id: "mcp", ...STATUS_SLOTS.mcp, component: McpStatusItem });
  return null;
}

/** 웰컴 창 아래 줄의 「MCP: 연결 n」 — 이 앱에 열린 세션 전부. */
export function McpConnections() {
  const T = useMcpStrings().welcome;
  const count = useMcpSessions().length;
  return <span data-testid="welcome-mcp">{T.connections(count)}</span>;
}
