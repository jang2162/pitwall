// 테이블 데이터의 문자열(14-m3-plan.md M3-17). 내용은 Fluent 리소스 extensions/i18n-default/i18n/*/db-table-data.ftl 에 있다. 여기
// 적은 id 가 ko · en 리소스에 모두 있는지는 src/i18n/resources.test.ts 가 본다. 거절 · 세션 알림 · 등급의 문구는 M3-19 의
// db-safety.ftl 것을 쓴다(safety/strings.ts).

import { useLocalization } from "@fluent/react";
import type { DbEditChangeKind, DbEditUndone } from "@pitwall/rpc-client";
import { useMemo } from "react";

import { type Translate, translator } from "../../i18n/localization";
import type { EditBlock } from "./table-data-store";

export function dbTableDataStrings(t: Translate) {
  const change: Record<DbEditChangeKind, (index: number) => string> = {
    update: (index) => t("db-table-data-change-update", { index }),
    insert: (index) => t("db-table-data-change-insert", { index }),
    delete: (index) => t("db-table-data-change-delete", { index }),
  };
  const undone: Record<DbEditUndone, string> = {
    edit: t("db-table-data-undone-edit"),
    savepoint: t("db-table-data-undone-savepoint"),
    transaction: t("db-table-data-undone-transaction"),
    unknown: t("db-table-data-undone-unknown"),
  };
  const block: Record<
    Exclude<EditBlock, "notConnected" | "loading" | "submitting">,
    (message: string) => string
  > = {
    readOnly: () => t("db-table-data-block-read-only"),
    noKey: () => t("db-table-data-block-no-key"),
    keyNotInResult: () => t("db-table-data-block-key-not-in-result"),
    failed: (message) => t("db-table-data-block-failed", { message }),
  };
  return {
    title: t("db-table-data-title"),
    tabTitle: (table: string) => t("db-table-data-tab-title", { table }),
    bar: {
      connect: t("db-table-data-connect"),
      disconnect: t("db-table-data-disconnect"),
      where: t("db-table-data-where"),
      wherePlaceholder: t("db-table-data-where-placeholder"),
      orderBy: t("db-table-data-order-by"),
      orderByPlaceholder: t("db-table-data-order-by-placeholder"),
      read: t("db-table-data-read"),
      readHint: t("db-table-data-read-hint"),
      addRow: t("db-table-data-add-row"),
      deleteRow: t("db-table-data-delete-row"),
      submit: t("db-table-data-submit"),
      revert: t("db-table-data-revert"),
      pending: (count: number) => t("db-table-data-pending", { count }),
      notConnected: t("db-table-data-not-connected"),
      disconnected: t("db-table-data-disconnected"),
      pendingBlocksRead: t("db-table-data-pending-blocks-read"),
      error: (message: string) => t("db-table-data-error", { message }),
      applied: (statements: number, rows: number) =>
        t("db-table-data-applied", { statements, rows }),
    },
    block,
    mark: {
      changed: t("db-table-data-mark-changed"),
      inserted: t("db-table-data-mark-inserted"),
      deleted: t("db-table-data-mark-deleted"),
    },
    edit: {
      label: (column: string) => t("db-table-data-edit-label", { column }),
      null: t("db-table-data-edit-null"),
      apply: t("db-table-data-edit-apply"),
      cancel: t("db-table-data-edit-cancel"),
      key: t("db-table-data-edit-key"),
      truncated: t("db-table-data-edit-truncated"),
      deleted: t("db-table-data-edit-deleted"),
      fkNull: t("db-table-data-fk-null"),
      fkPending: t("db-table-data-fk-pending"),
    },
    preview: {
      title: t("db-table-data-preview-title"),
      own: t("db-table-data-preview-own"),
      open: t("db-table-data-preview-open"),
      statements: (count: number) => t("db-table-data-preview-statements", { count }),
      apply: t("db-table-data-preview-apply"),
      cancel: t("db-table-data-preview-cancel"),
      applying: t("db-table-data-preview-applying"),
    },
    fail: {
      rowCount: (change: string, rows: number) =>
        t("db-table-data-fail-row-count", { change, rows }),
      changed: t("db-table-data-fail-changed"),
      db: (message: string) => t("db-table-data-fail-db", { message }),
      invalid: (reason: string, column: string) =>
        t("db-table-data-fail-invalid", { reason, column }),
      undone,
      change,
    },
    close: {
      title: t("db-table-data-close-title"),
      body: (table: string, count: number) => t("db-table-data-close-body", { table, count }),
      confirm: t("db-table-data-close-confirm"),
    },
    actions: {
      open: t("db-table-data-action-open"),
      submit: t("db-table-data-action-submit"),
      revert: t("db-table-data-action-revert"),
      addRow: t("db-table-data-action-add-row"),
      deleteRow: t("db-table-data-action-delete-row"),
    },
  };
}

export type DbTableDataStrings = ReturnType<typeof dbTableDataStrings>;

/** 지금 언어의 문자열. 언어가 바뀌면 새 객체다. */
export function useDbTableDataStrings(): DbTableDataStrings {
  const { l10n } = useLocalization();
  return useMemo(() => dbTableDataStrings(translator(l10n)), [l10n]);
}
