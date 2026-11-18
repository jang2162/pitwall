// MCP 화면이 보이는 문자열(16-m5-plan M5-10 · M5-13). 내용은 extensions/i18n-default/i18n/*/mcp.ftl 이다 — 프로젝트 창 · 웰컴 창(그리고 뒤에
// 설정 창)이 함께 쓰므로 창 문자열(layout/strings.ts · welcome/strings.ts)에 넣지 않고 여기 둔다. 「에이전트 활동」 패널(M5-13) ·
// 승인 다이얼로그(M5-14) · 설정 > MCP(M5-15)가 같은 파일에 묶음을 더한다. id 가 두 언어에 모두 있는지는 src/i18n/resources.test.ts 가 본다.

import { useLocalization } from "@fluent/react";
import { useMemo } from "react";

import type {
  AuditApproval,
  AuditOutcome,
  McpSettingRefusal,
  McpSettingsFileKind,
  Tier,
} from "@pitwall/rpc-client";

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
    },
    activity: {
      approvalsTitle: t("mcp-activity-approvals-title"),
      approvalsEmpty: t("mcp-activity-approvals-empty"),
      approvalsMore: (count: number) => t("mcp-activity-approvals-more", { count }),
      approvalsOpen: (agent: string, tool: string) =>
        t("mcp-activity-approvals-open", { agent, tool }),
      logTitle: t("mcp-activity-log-title"),
      logScope: t("mcp-activity-log-scope"),
      column: {
        time: t("mcp-activity-column-time"),
        client: t("mcp-activity-column-client"),
        tool: t("mcp-activity-column-tool"),
        args: t("mcp-activity-column-args"),
        outcome: t("mcp-activity-column-outcome"),
        duration: t("mcp-activity-column-duration"),
        approval: t("mcp-activity-column-approval"),
      },
      outcome: {
        ok: t("mcp-activity-outcome-ok"),
        error: t("mcp-activity-outcome-error"),
        denied: t("mcp-activity-outcome-denied"),
        started: t("mcp-activity-outcome-started"),
      } satisfies Record<AuditOutcome, string>,
      approval: {
        auto: t("mcp-activity-approval-auto"),
        once: t("mcp-activity-approval-once"),
        session: t("mcp-activity-approval-session"),
        window: t("mcp-activity-approval-window"),
        policy: t("mcp-activity-approval-policy"),
      } satisfies Record<AuditApproval, string>,
      approvalNone: t("mcp-activity-approval-none"),
      noProject: t("mcp-activity-no-project"),
      duration: (ms: number) => t("mcp-activity-duration", { ms }),
      filter: {
        outcome: t("mcp-activity-filter-outcome"),
        tool: t("mcp-activity-filter-tool"),
        client: t("mcp-activity-filter-client"),
        allOutcomes: t("mcp-activity-filter-all-outcomes"),
        allTools: t("mcp-activity-filter-all-tools"),
        allClients: t("mcp-activity-filter-all-clients"),
      },
      loading: t("mcp-activity-loading"),
      empty: t("mcp-activity-empty"),
      emptyFiltered: t("mcp-activity-empty-filtered"),
      loadFailed: (message: string) => t("mcp-activity-load-failed", { message }),
      clear: t("mcp-activity-clear"),
      clearTitle: t("mcp-activity-clear-title"),
      clearBody: t("mcp-activity-clear-body"),
      clearConfirm: t("mcp-activity-clear-confirm"),
      clearCancel: t("mcp-activity-clear-cancel"),
      clearFailed: t("mcp-activity-clear-failed"),
    },
    welcome: {
      connections: (count: number) => t("mcp-welcome-connections", { count }),
    },
    approval: {
      title: t("mcp-approval-title"),
      notThisProject: t("mcp-approval-not-this-project"),
      agent: t("mcp-approval-agent"),
      unknownAgent: t("mcp-approval-unknown-agent"),
      cli: t("mcp-approval-cli"),
      tool: t("mcp-approval-tool"),
      tierLabel: t("mcp-approval-tier"),
      tier: {
        read: t("mcp-approval-tier-read"),
        write: t("mcp-approval-tier-write"),
        exec: t("mcp-approval-tier-exec"),
        dangerous: t("mcp-approval-tier-dangerous"),
      } satisfies Record<Tier, string>,
      summary: t("mcp-approval-summary"),
      agentText: t("mcp-approval-agent-text"),
      changes: (count: number) => t("mcp-approval-changes", { count }),
      noChanges: t("mcp-approval-no-changes"),
      file: {
        added: t("mcp-approval-file-added"),
        deleted: t("mcp-approval-file-deleted"),
        modified: t("mcp-approval-file-modified"),
      },
      unchangedLines: t("mcp-approval-unchanged-lines"),
      textCut: (count: number) => t("mcp-approval-text-cut", { count }),
      invisible: t("mcp-approval-invisible"),
      sql: {
        connection: t("mcp-approval-sql-connection"),
        file: t("mcp-approval-sql-file"),
        irreversible: t("mcp-approval-sql-irreversible"),
        statements: (count: number) => t("mcp-approval-sql-statements", { count }),
        statement: (n: number, line: number) => t("mcp-approval-sql-statement", { n, line }),
        foldedReads: (count: number) => t("mcp-approval-sql-folded-reads", { count }),
      },
      remaining: (time: string) => t("mcp-approval-remaining", { time }),
      position: (index: number, total: number) => t("mcp-approval-position", { index, total }),
      locked: t("mcp-approval-locked"),
      deny: t("mcp-approval-deny"),
      once: t("mcp-approval-once"),
      session: t("mcp-approval-session"),
      window: t("mcp-approval-window"),
      notifyTitle: t("mcp-approval-notify-title"),
      notifyBody: (agent: string, tool: string) => t("mcp-approval-notify-body", { agent, tool }),
      notifyAgent: t("mcp-approval-notify-agent"),
    },
    /** 설정 > MCP(M5-15, settings/mcp-section.tsx). */
    settings: {
      title: t("mcp-settings-title"),
      intro: t("mcp-settings-intro"),
      loading: t("mcp-settings-loading"),
      failed: (error: string) => t("mcp-settings-failed", { error }),
      openFile: t("mcp-settings-open-file"),
      openFailed: (path: string) => t("mcp-settings-open-failed", { path }),
      warning: (warning: string) => t("mcp-settings-warning", { warning }),
      preset: t("mcp-settings-preset"),
      presets: {
        "read-only": t("mcp-settings-preset-read-only"),
        standard: t("mcp-settings-preset-standard"),
        full: t("mcp-settings-preset-full"),
      },
      autoApprove: t("mcp-settings-auto-approve"),
      autoApproveNote: t("mcp-settings-auto-approve-note"),
      retention: t("mcp-settings-retention"),
      retentionUnit: t("mcp-settings-retention-unit"),
      retentionHint: (max: number, days: number) => t("mcp-settings-retention-hint", { max, days }),
      apply: t("mcp-settings-apply"),
      reset: t("mcp-settings-reset"),
      groups: t("mcp-settings-groups"),
      group: {
        default: t("mcp-settings-group-default"),
        on: t("mcp-settings-group-on"),
        off: t("mcp-settings-group-off"),
      },
      tools: t("mcp-settings-tools"),
      search: t("mcp-settings-search"),
      toolsCount: (count: number) => t("mcp-settings-tools-count", { count }),
      noTools: t("mcp-settings-no-tools"),
      column: {
        on: t("mcp-settings-column-on"),
        name: t("mcp-settings-column-name"),
        tier: t("mcp-settings-column-tier"),
        group: t("mcp-settings-column-group"),
        calls: t("mcp-settings-column-calls"),
        why: t("mcp-settings-column-why"),
      },
      schemaShow: t("mcp-settings-schema-show"),
      schemaHide: t("mcp-settings-schema-hide"),
      rule: {
        preset: (key: string) => t("mcp-settings-rule-preset", { key }),
        tool: t("mcp-settings-rule-tool"),
        glob: (key: string) => t("mcp-settings-rule-glob", { key }),
        group: (key: string) => t("mcp-settings-rule-group", { key }),
        shared: t("mcp-settings-rule-shared"),
      },
      project: t("mcp-settings-project"),
      projectNone: t("mcp-settings-project-none"),
      projectFiles: t("mcp-settings-project-files"),
      file: {
        shared: t("mcp-settings-file-shared"),
        local: t("mcp-settings-file-local"),
        personal: t("mcp-settings-file-personal"),
      } satisfies Record<McpSettingsFileKind, string>,
      fileMissing: t("mcp-settings-file-missing"),
      projectOverrides: t("mcp-settings-project-overrides"),
      projectNoOverrides: t("mcp-settings-project-no-overrides"),
      overrideFrom: (file: string) => t("mcp-settings-override-from", { file }),
      projectAutoApprove: t("mcp-settings-project-auto-approve"),
      projectAutoApproveNone: t("mcp-settings-project-auto-approve-none"),
      projectNote: t("mcp-settings-project-note"),
      projectSets: t("mcp-settings-project-sets"),
      inProject: (state: string, why: string) => t("mcp-settings-in-project", { state, why }),
      differsBadge: t("mcp-settings-differs-badge"),
      stateOn: t("mcp-settings-state-on"),
      stateOff: t("mcp-settings-state-off"),
      register: t("mcp-settings-register"),
      registerIntro: t("mcp-settings-register-intro"),
      registerClient: {
        claudeCode: t("mcp-settings-register-claude-code"),
        codex: t("mcp-settings-register-codex"),
        cursor: t("mcp-settings-register-cursor"),
      },
      copy: t("mcp-settings-copy"),
      copied: t("mcp-settings-copied"),
      copyFailed: (error: string) => t("mcp-settings-copy-failed", { error }),
      refused: {
        unknownPreset: t("mcp-settings-refused-unknown-preset"),
        dangerousAutoApprove: t("mcp-settings-refused-dangerous"),
        unknownTier: t("mcp-settings-refused-unknown-tier"),
        unknownGroup: t("mcp-settings-refused-unknown-group"),
        unknownTool: t("mcp-settings-refused-unknown-tool"),
        badRetentionDays: t("mcp-settings-refused-retention"),
        badValue: t("mcp-settings-refused-bad-value"),
      } satisfies Record<McpSettingRefusal, string>,
    },
  };
}

export type McpStrings = ReturnType<typeof mcpStrings>;

/** 지금 언어의 문자열. 언어가 바뀌면 새 객체다. */
export function useMcpStrings(): McpStrings {
  const { l10n } = useLocalization();
  return useMemo(() => mcpStrings(translator(l10n)), [l10n]);
}
