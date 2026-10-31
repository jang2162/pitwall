// DB 탐색기의 문자열(14-m3-plan.md M3-13). 내용은 Fluent 리소스 extensions/i18n-default/i18n/*/db-explorer.ftl 에 있다.
// 여기 적은 id 가 ko · en 리소스에 모두 있는지는 src/i18n/resources.test.ts 가 본다.
//
// 상태 표(`ConnectionBadge`)는 `Record<…>` 로 적는다 — 상태를 더하면 여기서 타입 오류가 난다.

import { useLocalization } from "@fluent/react";
import { useMemo } from "react";

import { type Translate, translator } from "../../i18n/localization";
import type { BrowsedKind, ConnectionBadge, DetailKind } from "./explorer-model";

export function dbExplorerStrings(t: Translate) {
  const status: Record<ConnectionBadge, string> = {
    idle: t("db-explorer-status-idle"),
    connecting: t("db-explorer-status-connecting"),
    connected: t("db-explorer-status-connected"),
    lost: t("db-explorer-status-lost"),
    configChanged: t("db-explorer-status-config-changed"),
    secretRequired: t("db-explorer-status-secret-required"),
    approvalRequired: t("db-explorer-status-approval-required"),
    untrusted: t("db-explorer-status-untrusted"),
    invalid: t("db-explorer-status-invalid"),
  };
  const groups: Record<BrowsedKind, string> = {
    table: t("db-explorer-group-tables"),
    view: t("db-explorer-group-views"),
  };
  const details: Record<DetailKind, string> = {
    columns: t("db-explorer-detail-columns"),
    indexes: t("db-explorer-detail-indexes"),
    foreignKeys: t("db-explorer-detail-foreign-keys"),
  };
  return {
    label: t("db-explorer-label"),
    toolbar: {
      add: t("db-explorer-add"),
      refresh: t("db-explorer-refresh"),
      find: t("db-explorer-find"),
      findPlaceholder: t("db-explorer-find-placeholder"),
      findClear: t("db-explorer-find-clear"),
    },
    empty: t("db-explorer-empty"),
    untrusted: t("db-explorer-untrusted"),
    listFailed: (error: string) => t("db-explorer-list-failed", { error }),
    configProblem: (problem: string) => t("db-explorer-config-problem", { problem }),
    stateProblem: (problem: string) => t("db-explorer-state-problem", { problem }),
    status,
    lostHint: (message: string) => t("db-explorer-status-lost-hint", { message }),
    configChangedHint: t("db-explorer-status-config-changed-hint"),
    groups,
    details,
    filtered: (filter: string) => t("db-explorer-filtered", { filter }),
    loading: t("db-explorer-loading"),
    more: t("db-explorer-more"),
    none: t("db-explorer-none"),
    failed: (error: string) => t("db-explorer-failed", { error }),
    item: {
      notNull: t("db-explorer-not-null"),
      primaryKey: t("db-explorer-primary-key"),
      unique: t("db-explorer-unique"),
      partial: t("db-explorer-partial"),
    },
    menu: {
      newConsole: t("db-explorer-menu-new-console"),
      ddl: t("db-explorer-menu-ddl"),
      /** `db.explorer.reveal` 의 이름(인자가 있어야 도는 액션이라 액션 찾기에는 보이지 않는다). */
      reveal: t("db-explorer-menu-reveal"),
      rowCount: t("db-explorer-menu-row-count"),
      copyName: t("db-explorer-menu-copy-name"),
      tableData: t("db-explorer-menu-table-data"),
      openRedis: t("db-explorer-menu-open-redis"),
      edit: t("db-explorer-menu-edit"),
      delete: t("db-explorer-menu-delete"),
      connect: t("db-explorer-menu-connect"),
      disconnect: t("db-explorer-menu-disconnect"),
      rowCountOtherDatabase: t("db-explorer-row-count-other-database"),
    },
    deleteConfirm: {
      title: (name: string) => t("db-explorer-delete-title", { name }),
      body: t("db-explorer-delete-body"),
      confirm: t("db-explorer-delete-confirm"),
    },
    rowCount: {
      title: t("db-explorer-row-count-title"),
      body: (name: string, count: string) => t("db-explorer-row-count-body", { name, count }),
    },
    actionFailed: (action: string, error: string) =>
      t("db-explorer-action-failed", { action, error }),
    copied: (name: string) => t("db-explorer-copied", { name }),
    ddl: {
      title: (name: string) => t("db-explorer-ddl-title", { name }),
      read: t("db-explorer-ddl-read"),
      reload: t("db-explorer-ddl-reload"),
      restored: t("db-explorer-ddl-restored"),
    },
  };
}

export type DbExplorerStrings = ReturnType<typeof dbExplorerStrings>;

export function useDbExplorerStrings(): DbExplorerStrings {
  const { l10n } = useLocalization();
  return useMemo(() => dbExplorerStrings(translator(l10n)), [l10n]);
}
