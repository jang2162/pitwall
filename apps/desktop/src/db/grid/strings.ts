// DB 결과 그리드가 보여 주는 문자열. 내용은 Fluent 리소스 extensions/i18n-default/i18n/*/db-grid.ftl 에 있다. 여기 적은 id 가
// ko · en 리소스에 모두 있는지는 src/i18n/resources.test.ts 가 본다.

import { useLocalization } from "@fluent/react";
import { useMemo } from "react";

import { type Translate, translator } from "../../i18n/localization";
import type { CopyFormat } from "./copy";

export function dbGridStrings(t: Translate) {
  const format: Record<CopyFormat, string> = {
    csv: t("db-grid-format-csv"),
    json: t("db-grid-format-json"),
    insert: t("db-grid-format-insert"),
    tsv: t("db-grid-format-tsv"),
  };
  return {
    label: t("db-grid-label"),
    status: {
      loading: (count: number) => t("db-grid-loading", { count }),
      rows: (count: number) => t("db-grid-rows", { count }),
      filtered: (visible: number, total: number) => t("db-grid-filtered", { visible, total }),
      elapsed: (ms: number) => t("db-grid-elapsed", { ms }),
      capped: (count: number) => t("db-grid-capped", { count }),
      cappedHint: t("db-grid-capped-hint"),
      affected: (count: number) => t("db-grid-affected", { count }),
      cancelled: t("db-grid-cancelled"),
      failed: (message: string) => t("db-grid-failed", { message }),
      noColumns: t("db-grid-no-columns"),
    },
    filter: {
      placeholder: t("db-grid-filter"),
      allColumns: t("db-grid-filter-all-columns"),
      clear: t("db-grid-filter-clear"),
    },
    copy: {
      copy: t("db-grid-copy"),
      copyAs: t("db-grid-copy-as"),
      copyAsTitle: t("db-grid-copy-as-title"),
      format,
      copied: (f: CopyFormat, count: number) =>
        t("db-grid-copied", { format: format[f] ?? f, count }),
      nothing: t("db-grid-copy-nothing"),
      truncated: (row: number, column: string) => t("db-grid-copy-truncated", { row, column }),
      unsupported: t("db-grid-copy-unsupported"),
      nul: t("db-grid-copy-nul"),
      tooLarge: t("db-grid-copy-too-large"),
      failed: (message: string) => t("db-grid-copy-failed", { message }),
    },
    detail: {
      title: t("db-grid-detail"),
      show: t("db-grid-detail-show"),
      close: t("db-grid-detail-close"),
      none: t("db-grid-detail-none"),
      text: t("db-grid-detail-text"),
      json: t("db-grid-detail-json"),
      hex: t("db-grid-detail-hex"),
      jsonInvalid: t("db-grid-detail-json-invalid"),
      truncated: (size: string, received: string) =>
        t("db-grid-detail-truncated", { size, received }),
      more: (count: number) => t("db-grid-detail-more", { count }),
      expand: t("db-grid-detail-expand"),
      collapse: t("db-grid-detail-collapse"),
    },
  };
}

export type DbGridStrings = ReturnType<typeof dbGridStrings>;

export function useDbGridStrings(): DbGridStrings {
  const { l10n } = useLocalization();
  return useMemo(() => dbGridStrings(translator(l10n)), [l10n]);
}
