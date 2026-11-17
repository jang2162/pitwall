// MCP 화면이 보이는 문자열(16-m5-plan M5-10). 내용은 extensions/i18n-default/i18n/*/mcp.ftl 이다 — 프로젝트 창 · 웰컴 창(그리고 뒤에
// 설정 창)이 함께 쓰므로 창 문자열(layout/strings.ts · welcome/strings.ts)에 넣지 않고 여기 둔다. 「에이전트 활동」 패널(M5-13) ·
// 승인 다이얼로그(M5-14) · 설정 > MCP(M5-15)가 같은 파일에 묶음을 더한다. id 가 두 언어에 모두 있는지는 src/i18n/resources.test.ts 가 본다.

import { useLocalization } from "@fluent/react";
import { useMemo } from "react";

import { type Translate, translator } from "../i18n/localization";

export function mcpStrings(t: Translate) {
  return {
    status: {
      label: (count: number) => t("mcp-status-label", { count }),
      hint: (count: number) => t("mcp-status-hint", { count }),
    },
    sessions: {
      title: t("mcp-sessions-title"),
      empty: t("mcp-sessions-empty"),
      unknownClient: t("mcp-sessions-unknown-client"),
      started: (time: string) => t("mcp-sessions-started", { time }),
      calls: (count: number) => t("mcp-sessions-calls", { count }),
      close: t("mcp-sessions-close"),
    },
    welcome: {
      connections: (count: number) => t("mcp-welcome-connections", { count }),
    },
  };
}

export type McpStrings = ReturnType<typeof mcpStrings>;

/** 지금 언어의 문자열. 언어가 바뀌면 새 객체다. */
export function useMcpStrings(): McpStrings {
  const { l10n } = useLocalization();
  return useMemo(() => mcpStrings(translator(l10n)), [l10n]);
}
