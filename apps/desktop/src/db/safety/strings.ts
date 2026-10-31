// DB 안전장치 화면 부품의 문자열(14-m3-plan.md M3-19). 내용은 Fluent 리소스 extensions/i18n-default/i18n/*/db-safety.ftl 에 있다.
// 여기 적은 id 가 ko · en 리소스에 모두 있는지는 src/i18n/resources.test.ts 가 본다.
//
// 까닭(`RiskReason`) · 등급(`RiskGrade`) · 거절 까닭 표는 `Record<…>` 로 적는다 — Host 가 갈래를 더하면(생성 타입이 바뀌면) 여기서
// 타입 오류가 난다. 문구가 빠진 채로 화면에 이름이 그대로 나가지 않게 한다.

import { useLocalization } from "@fluent/react";
import type { RiskGrade, RiskReason } from "@pitwall/rpc-client";
import { useMemo } from "react";

import { type Translate, translator } from "../../i18n/localization";

/** Host 가 실행 · 연결을 거절한 까닭(`PERMISSION_DENIED` · `CONFLICT` · `INVALID_PARAMS` 의 `data.reason`, crates/pw-db). */
export type DbRefusalReason =
  | "readOnlyConnection"
  | "untrusted"
  | "notProjectWindow"
  | "notAQuery"
  | "transactionStatement"
  | "busy"
  | "configChanged"
  | "transactionOpen";

export function dbSafetyStrings(t: Translate) {
  return {
    label: {
      production: t("db-label-production"),
      readOnly: t("db-label-read-only"),
    },
    tls: {
      unverified: t("db-tls-unverified"),
      unverifiedHint: t("db-tls-unverified-hint"),
      plaintext: t("db-tls-plaintext"),
      plaintextHint: t("db-tls-plaintext-hint"),
    },
    grade: {
      read: t("db-grade-read"),
      write: t("db-grade-write"),
      unboundedWrite: t("db-grade-unbounded-write"),
      ddl: t("db-grade-ddl"),
      destructive: t("db-grade-destructive"),
    } satisfies Record<RiskGrade, string>,
    confirm: {
      title: {
        read: t("db-confirm-title-read"),
        write: t("db-confirm-title-write"),
        unboundedWrite: t("db-confirm-title-unbounded-write"),
        ddl: t("db-confirm-title-ddl"),
        destructive: t("db-confirm-title-destructive"),
      } satisfies Record<RiskGrade, string>,
      grade: (grade: string) => t("db-confirm-grade", { grade }),
      connection: (connection: string) => t("db-confirm-connection", { connection }),
      production: (connection: string) => t("db-confirm-production", { connection }),
      namePrompt: (connection: string) => t("db-confirm-name-prompt", { connection }),
      name: t("db-confirm-name"),
      nameMismatch: t("db-confirm-name-mismatch"),
      statements: (count: number) => t("db-confirm-statements", { count }),
      more: (count: number) => t("db-confirm-more", { count }),
      unparsed: t("db-confirm-unparsed"),
      statement: (index: number) => t("db-confirm-statement", { index }),
      cancel: t("db-confirm-cancel"),
      run: t("db-confirm-run"),
      rejected: {
        hashMismatch: t("db-confirm-rejected-hash-mismatch"),
        gradeTooLow: t("db-confirm-rejected-grade-too-low"),
        nameMismatch: t("db-confirm-rejected-name-mismatch"),
        other: t("db-confirm-rejected-other"),
      },
    },
    effect: {
      implicitCommit: t("db-effect-implicit-commit"),
    },
    /** `3002`: 취소가 트랜잭션 전체를 되돌린다 — M2-20 의 확인(`ConfirmDialog`)에 그대로 싣는다. */
    cancelRollback: {
      title: t("db-cancel-rollback-title"),
      body: t("db-cancel-rollback-body"),
      confirm: t("db-cancel-rollback-confirm"),
    },
    reason: {
      unparsed: t("db-reason-unparsed"),
      tooLarge: t("db-reason-too-large"),
      tooDeep: t("db-reason-too-deep"),
      drop: t("db-reason-drop"),
      truncate: t("db-reason-truncate"),
      copyProgram: t("db-reason-copy-program"),
      ddl: t("db-reason-ddl"),
      missingWhere: t("db-reason-missing-where"),
      unboundedWhere: t("db-reason-unbounded-where"),
      multiTable: t("db-reason-multi-table"),
      orReplace: t("db-reason-or-replace"),
      merge: t("db-reason-merge"),
      dml: t("db-reason-dml"),
      lockingRead: t("db-reason-locking-read"),
      sideEffectFunction: t("db-reason-side-effect-function"),
      serverAdmin: t("db-reason-server-admin"),
      replicationReset: t("db-reason-replication-reset"),
      selectInto: t("db-reason-select-into"),
      transactionControl: t("db-reason-transaction-control"),
      sessionState: t("db-reason-session-state"),
      lock: t("db-reason-lock"),
      procedural: t("db-reason-procedural"),
      attach: t("db-reason-attach"),
      pragma: t("db-reason-pragma"),
      maintenance: t("db-reason-maintenance"),
      dataTransfer: t("db-reason-data-transfer"),
      other: t("db-reason-other"),
    } satisfies Record<RiskReason, string>,
    refusal: {
      readOnlyConnection: () => t("db-refusal-read-only-connection"),
      untrusted: () => t("db-refusal-untrusted"),
      notProjectWindow: () => t("db-refusal-not-project-window"),
      notAQuery: () => t("db-refusal-not-a-query"),
      transactionStatement: (message: string) => t("db-refusal-transaction-statement", { message }),
      busy: () => t("db-refusal-busy"),
      configChanged: () => t("db-refusal-config-changed"),
      transactionOpen: () => t("db-refusal-transaction-open"),
    } satisfies Record<DbRefusalReason, (message: string) => string>,
    notice: {
      rolledBack: (message: string) => t("db-notice-rolled-back", { message }),
      committedImplicitly: t("db-notice-committed-implicitly"),
      transactionEnded: t("db-notice-transaction-ended"),
      transactionEndedMessage: (message: string) =>
        t("db-notice-transaction-ended-message", { message }),
      transactionUnknown: (message: string) => t("db-notice-transaction-unknown", { message }),
      transactionOpened: t("db-notice-transaction-opened"),
      connectionLost: t("db-notice-connection-lost"),
      connectionLostRolledBack: t("db-notice-connection-lost-rolled-back"),
      connectionLostOutcomeUnknown: t("db-notice-connection-lost-outcome-unknown"),
      configChanged: t("db-notice-config-changed"),
      unattended: (until: string) => t("db-notice-unattended", { until }),
      unattendedRolledBack: t("db-notice-unattended-rolled-back"),
    },
    status: {
      mode: {
        auto: t("db-status-mode-auto"),
        manual: t("db-status-mode-manual"),
      },
      transaction: (elapsed: string) => t("db-status-transaction", { elapsed }),
      failed: (elapsed: string) => t("db-status-failed", { elapsed }),
      disconnected: t("db-status-disconnected"),
      configChanged: t("db-status-config-changed"),
      hint: (connection: string, mode: string) => t("db-status-hint", { connection, mode }),
    },
  };
}

export type DbSafetyStrings = ReturnType<typeof dbSafetyStrings>;

/** 지금 언어의 문자열. 언어가 바뀌면 새 객체다. */
export function useDbSafetyStrings(): DbSafetyStrings {
  const { l10n } = useLocalization();
  return useMemo(() => dbSafetyStrings(translator(l10n)), [l10n]);
}
