// DB 접근 묻기(access-prompt.tsx)의 문자열. 내용은 Fluent 리소스 extensions/i18n-default/i18n/*/db-safety.ftl 의 「DB 접근 묻기」
// 절에 있다. 갈래(`SecretMissing` · 확인 종류)는 `Record<…>` 로 적는다 — Host 가 갈래를 더하면 여기서 타입 오류가 난다.

import { useLocalization } from "@fluent/react";
import type { DbApprovalRequest, SecretMissing } from "@pitwall/rpc-client";
import { useMemo } from "react";

import { type Translate, translator } from "../../i18n/localization";

export function dbAccessStrings(t: Translate) {
  const reason: Record<SecretMissing, string> = {
    missing: t("db-access-secret-missing"),
    targetChanged: t("db-access-secret-target-changed"),
    unavailable: t("db-access-secret-unavailable"),
  };
  const approveTitle: Record<DbApprovalRequest["kind"], string> = {
    env: t("db-access-approve-title-env"),
    file: t("db-access-approve-title-file"),
    tunnel: t("db-access-approve-title-tunnel"),
    certFiles: t("db-access-approve-title-cert-files"),
  };
  return {
    cancel: t("db-access-cancel"),
    secret: {
      title: (connection: string) => t("db-access-secret-title", { connection }),
      reason,
      label: t("db-access-secret-label"),
      save: t("db-access-secret-save"),
    },
    approve: {
      title: approveTitle,
      hint: t("db-access-approve-hint"),
      run: t("db-access-approve"),
    },
    previous: t("db-access-previous"),
    now: t("db-access-now"),
    field: {
      variable: t("db-access-variable"),
      path: t("db-access-path"),
      destination: t("db-access-destination"),
      driver: t("db-access-driver"),
      host: t("db-access-host"),
      port: t("db-access-port"),
      user: t("db-access-user"),
      tunnel: t("db-access-tunnel"),
      sslmode: t("db-access-sslmode"),
      sslrootcert: t("db-access-sslrootcert"),
      sslcert: t("db-access-sslcert"),
      sslkey: t("db-access-sslkey"),
    },
    none: t("db-access-none"),
    failed: (error: string) => t("db-access-failed", { error }),
  };
}

export type DbAccessStrings = ReturnType<typeof dbAccessStrings>;

export function useDbAccessStrings(): DbAccessStrings {
  const { l10n } = useLocalization();
  return useMemo(() => dbAccessStrings(translator(l10n)), [l10n]);
}
