// DB 연결 편집 다이얼로그의 문자열(14-m3-plan.md M3-14). 내용은 Fluent 리소스 extensions/i18n-default/i18n/*/db-connection.ftl 에
// 있다. 여기 적은 id 가 ko · en 리소스에 모두 있는지는 src/i18n/resources.test.ts 가 본다.

import { useLocalization } from "@fluent/react";
import type { SslMode, TlsStatus } from "@pitwall/rpc-client";
import { useMemo } from "react";

import { type Translate, translator } from "../../i18n/localization";
import type { TabId } from "./form";

export function dbConnectionStrings(t: Translate) {
  return {
    actions: {
      add: t("action-db-connection-add"),
      edit: t("action-db-connection-edit"),
    },
    title: {
      new: t("db-conn-title-new"),
      edit: (name: string) => t("db-conn-title-edit", { name }),
    },
    loading: t("db-conn-loading"),
    loadFailed: (message: string) => t("db-conn-load-failed", { message }),
    notFound: (id: string) => t("db-conn-not-found", { id }),
    invalid: (message: string) => t("db-conn-invalid", { message }),
    configProblem: (problem: string) => t("db-conn-config-problem", { problem }),
    tab: {
      general: t("db-conn-tab-general"),
      ssh: t("db-conn-tab-ssh"),
      ssl: t("db-conn-tab-ssl"),
      options: t("db-conn-tab-options"),
    } satisfies Record<TabId, string>,
    field: {
      id: t("db-conn-field-id"),
      idHint: t("db-conn-field-id-hint"),
      name: t("db-conn-field-name"),
      driver: t("db-conn-field-driver"),
      driverName: (driver: string) => t("db-conn-driver", { driver }),
      host: t("db-conn-field-host"),
      hostTunnel: t("db-conn-field-host-tunnel"),
      port: t("db-conn-field-port"),
      portDefault: (port: string) => t("db-conn-field-port-default", { port }),
      database: t("db-conn-field-database"),
      databaseRedis: t("db-conn-field-database-redis"),
      user: t("db-conn-field-user"),
      path: t("db-conn-field-path"),
      pathHint: t("db-conn-field-path-hint"),
      required: (field: string) => t("db-conn-required", { field }),
    },
    password: {
      label: t("db-conn-password"),
      source: {
        none: t("db-conn-password-source-none"),
        keychain: t("db-conn-password-source-keychain"),
        env: t("db-conn-password-source-env"),
      },
      stored: t("db-conn-password-stored"),
      missing: t("db-conn-password-missing"),
      targetChanged: t("db-conn-password-target-changed"),
      unavailable: t("db-conn-password-unavailable"),
      afterApproval: t("db-conn-password-after-approval"),
      untrusted: t("db-conn-password-untrusted"),
      hint: t("db-conn-password-hint"),
      envVariable: t("db-conn-env-variable"),
      envHint: t("db-conn-env-hint"),
      stateProblem: (problem: string) => t("db-conn-state-problem", { problem }),
    },
    ssh: {
      use: t("db-conn-ssh-use"),
      host: t("db-conn-ssh-host"),
      pick: t("db-conn-ssh-pick"),
      direct: t("db-conn-ssh-direct"),
      directPlaceholder: t("db-conn-ssh-direct-placeholder"),
      noHosts: t("db-conn-ssh-no-hosts"),
      untrusted: t("db-conn-ssh-untrusted"),
      hint: t("db-conn-ssh-hint"),
    },
    ssl: {
      mode: t("db-conn-ssl-mode"),
      auto: t("db-conn-ssl-auto"),
      unverified: t("db-conn-ssl-unverified"),
      plaintext: t("db-conn-ssl-plaintext"),
      unsupported: (driver: string, mode: SslMode) =>
        t("db-conn-ssl-unsupported", { driver, mode }),
      unsupportedMysqlVerifyCa: t("db-conn-ssl-unsupported-mysql-verify-ca"),
      unsupportedChosen: t("db-conn-ssl-unsupported-chosen"),
      rootcert: t("db-conn-ssl-rootcert"),
      cert: t("db-conn-ssl-cert"),
      key: t("db-conn-ssl-key"),
      pathHint: t("db-conn-ssl-path-hint"),
    },
    options: {
      readOnly: t("db-conn-read-only"),
      readOnlyHint: t("db-conn-read-only-hint"),
      production: t("db-conn-production"),
      productionHint: t("db-conn-production-hint"),
      color: t("db-conn-color"),
      colorNone: t("db-conn-color-none"),
      /** 테마 토큰 `db.tag.<색>`(M3-19 `DB_TAG_TOKENS`)의 이름. Fluent 의 선택 키에는 `.` 이 들 수 없어 색 이름만 넘긴다. */
      colorTag: (tag: string) => t("db-conn-color-tag", { tag: tag.replace(/^db\.tag\./, "") }),
      connectTimeout: t("db-conn-connect-timeout"),
      connectTimeoutHint: t("db-conn-connect-timeout-hint"),
    },
    location: {
      label: t("db-conn-location"),
      shared: (id: string) => t("db-conn-location-shared", { id }),
      local: (id: string) => t("db-conn-location-local", { id }),
      localHint: t("db-conn-location-local-hint"),
    },
    button: {
      test: t("db-conn-test"),
      testing: t("db-conn-testing"),
      save: t("db-conn-save"),
      saving: t("db-conn-saving"),
      cancel: t("db-conn-cancel"),
    },
    test: {
      ok: (version: string) => t("db-conn-test-ok", { version }),
      okNoVersion: t("db-conn-test-ok-no-version"),
      tls: (tls: TlsStatus) => t("db-conn-test-tls", { tls }),
      draft: t("db-conn-test-draft"),
      failed: (message: string) => t("db-conn-test-failed", { message }),
      secret: t("db-conn-test-secret"),
      approval: (kind: string) => t("db-conn-test-approval", { kind }),
    },
    saveFailed: (message: string) => t("db-conn-save-failed", { message }),
    secretFailed: (message: string) => t("db-conn-secret-failed", { message }),
    saved: (name: string) => t("db-conn-saved", { name }),
  };
}

export type DbConnectionStrings = ReturnType<typeof dbConnectionStrings>;

/** 지금 언어의 문자열. 언어가 바뀌면 새 객체다. */
export function useDbConnectionStrings(): DbConnectionStrings {
  const { l10n } = useLocalization();
  return useMemo(() => dbConnectionStrings(translator(l10n)), [l10n]);
}
