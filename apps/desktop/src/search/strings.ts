// 전체에서 찾기(⌘⇧F 팝업 · 「검색 결과」 툴 윈도우 · 파일 트리의 「이 폴더에서 찾기」)가 보여 주는 문자열. 내용은 Fluent 리소스
// (extensions/i18n-default/i18n/*/project.ftl 의 「전체에서 찾기」 절)에 있다. 여기 적은 id 가 ko · en 리소스에 모두 있는지는
// src/i18n/resources.test.ts 가 본다.

import { useLocalization } from "@fluent/react";
import { useMemo } from "react";

import { type Translate, translator } from "../i18n/localization";

export function findStrings(t: Translate) {
  return {
    action: t("action-find-in-files"),
    placeholder: t("find-placeholder"),
    matchCase: t("find-match-case"),
    wholeWord: t("find-whole-word"),
    regex: t("find-regex"),
    excludeVendored: t("find-exclude-vendored"),
    scopeProject: t("find-scope-project"),
    scopeFolder: t("find-scope-folder"),
    folder: t("find-folder-label"),
    mask: t("find-mask-label"),
    maskPlaceholder: t("find-mask-placeholder"),
    searching: t("find-searching"),
    summary: (lines: number, files: number) => t("find-summary", { lines, files }),
    more: t("find-more"),
    cancelled: t("find-cancelled"),
    failed: (message: string) => t("find-failed", { message }),
    empty: t("find-empty"),
    stop: t("find-stop"),
    pin: t("find-pin"),
    preview: t("find-preview-label"),
    previewLoading: t("find-preview-loading"),
    previewFailed: (message: string) => t("find-preview-failed", { message }),
    resultsEmpty: t("find-results-empty"),
    resultsTitle: (query: string) => t("find-results-title", { query }),
    inFolder: t("find-in-folder"),
  };
}

export type FindStrings = ReturnType<typeof findStrings>;

export function useFindStrings(): FindStrings {
  const { l10n } = useLocalization();
  return useMemo(() => findStrings(translator(l10n)), [l10n]);
}
