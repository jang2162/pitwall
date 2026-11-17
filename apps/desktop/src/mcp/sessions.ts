// MCP 세션 목록(`mcp.sessions`, 16-m5-plan M5-10)을 읽는 훅. 세션이 열리고 닫히면 Host 범위 이벤트 `mcp.sessionsChanged` 가
// 오고, 쿼리 무효화 규칙(rpc/host.tsx 의 `hostInvalidations`)이 이 쿼리를 다시 부른다. 이벤트 payload 는 수뿐이라(owner 만 읽는
// 세션 목록을 이벤트에 싣지 않는다 — crates/pw-mcp/src/sessions.rs 머리 주석) 목록은 늘 RPC 로 읽는다.
//
// 「에이전트 활동」 패널(M5-13)도 이 훅을 쓴다. 호출 수는 이벤트 없이 바뀐다 — 프로젝트 없는 호출의 `mcp.activity`(Host 범위)가
// 이 쿼리를 무효화하고, 프로젝트 범위의 `mcp.activity` 는 그 패널이 구독한다.

import type { McpSessionInfo } from "@pitwall/rpc-client";

import { useRpcQuery } from "../rpc/host";

/** 열린 세션 전부(시작 순서). 앱 없이 뜬 헤드리스 `pitwall mcp` 의 세션은 없다. */
export function useMcpSessions(): readonly McpSessionInfo[] {
  const sessions = useRpcQuery("mcp.sessions", null, { retry: false });
  return sessions.data ?? NONE;
}

const NONE: readonly McpSessionInfo[] = [];

/** 그 프로젝트를 세션 프로젝트로 가진 세션. */
export function sessionsOf(
  sessions: readonly McpSessionInfo[],
  projectId: string,
): McpSessionInfo[] {
  return sessions.filter((s) => s.projectId === projectId);
}
