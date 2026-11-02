// 포맷(15-m4-plan M4-13)이 보이는 문자열. 내용은 extensions/i18n-default/i18n/*/project.ftl 의 `lsp-format-…` 묶음이고, 프로젝트 창
// 문자열(src/layout/strings.ts)의 `editor.lspFormat` 으로 들어간다.

import type { Translate } from "../i18n/localization";
import type { FormatStrings } from "./format";

export function lspFormatStrings(t: Translate) {
  return {
    action: t("lsp-format-action"),
    noFormatter: t("lsp-format-no-formatter"),
    stale: t("lsp-format-stale"),
    timedOut: t("lsp-format-timed-out"),
    failed: (message: string) => t("lsp-format-failed", { message }),
    refused: (message: string) => t("lsp-format-refused", { message }),
  } satisfies FormatStrings & { action: string };
}

export type LspFormatStrings = ReturnType<typeof lspFormatStrings>;
