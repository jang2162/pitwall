// SQL 콘솔의 문자열(14-m3-plan.md M3-15). 내용은 Fluent 리소스 extensions/i18n-default/i18n/*/db-console.ftl 에 있다. 여기 적은 id 가
// ko · en 리소스에 모두 있는지는 src/i18n/resources.test.ts 가 본다. 등급 · 거절 · 세션 알림 · TX 모드의 문구는 M3-19 의
// db-safety.ftl 것을 쓴다(safety/strings.ts).

import { useLocalization } from "@fluent/react";
import { useMemo } from "react";

import { type Translate, translator } from "../../i18n/localization";

export function dbConsoleStrings(t: Translate) {
  return {
    title: t("db-console-title"),
    /** 탭 이름: 연결 이름이 있으면 그것. */
    tabTitle: (connection: string) => t("db-console-tab-title", { connection }),
    bar: {
      connection: t("db-console-connection"),
      noConnection: t("db-console-no-connection"),
      database: t("db-console-database"),
      databaseDefault: t("db-console-database-default"),
      schema: t("db-console-schema"),
      schemaDefault: t("db-console-schema-default"),
      connect: t("db-console-connect"),
      disconnect: t("db-console-disconnect"),
      run: t("db-console-run"),
      runHint: t("db-console-run-hint"),
      cancel: t("db-console-cancel"),
      cancelHint: t("db-console-cancel-hint"),
      explain: t("db-console-explain"),
      mode: t("db-console-mode"),
      commit: t("db-console-commit"),
      rollback: t("db-console-rollback"),
      notConnected: t("db-console-not-connected"),
      failed: t("db-console-failed"),
      disconnected: t("db-console-disconnected"),
      configChanged: t("db-console-config-changed"),
      chooseLocked: t("db-console-choose-locked"),
    },
    results: {
      label: t("db-console-results"),
      tab: (index: number) => t("db-console-result-tab", { index }),
      plan: t("db-console-plan"),
      empty: t("db-console-results-empty"),
      planCost: t("db-console-plan-cost"),
      planRows: t("db-console-plan-rows"),
      planEmpty: t("db-console-plan-empty"),
      resize: t("db-console-resize"),
    },
    close: {
      title: t("db-console-close-title"),
      failedTitle: t("db-console-close-failed-title"),
      body: t("db-console-close-body"),
      failedBody: t("db-console-close-failed-body"),
      cancel: t("db-console-close-cancel"),
      rollback: t("db-console-close-rollback"),
      commit: t("db-console-close-commit"),
    },
    actions: {
      open: t("db-console-action-open"),
      run: t("db-console-action-run"),
      runAll: t("db-console-action-run-all"),
      cancel: t("db-console-action-cancel"),
      explain: t("db-console-action-explain"),
    },
    error: {
      load: (message: string) => t("db-console-load-failed", { message }),
      reload: t("db-console-reload"),
      save: (message: string) => t("db-console-save-failed", { message }),
      other: (message: string) => t("db-console-error", { message }),
      nothingToRun: t("db-console-nothing-to-run"),
      explainOne: t("db-console-explain-one"),
    },
    unattended: {
      title: t("db-console-unattended-title"),
      body: (connection: string, since: string, minutes: number) =>
        t("db-console-unattended-body", { connection, since, minutes }),
      bodyUnknown: (connection: string) => t("db-console-unattended-body-unknown", { connection }),
    },
  };
}

export type DbConsoleStrings = ReturnType<typeof dbConsoleStrings>;

/** 지금 언어의 문자열. 언어가 바뀌면 새 객체다. */
export function useDbConsoleStrings(): DbConsoleStrings {
  const { l10n } = useLocalization();
  return useMemo(() => dbConsoleStrings(translator(l10n)), [l10n]);
}
