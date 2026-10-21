// 파일 트리(프로젝트 툴 윈도우)와 「다음에서 열기」 팝업의 「프로젝트 뷰」 항목이 보여 주는 문자열. 내용은 Fluent 리소스
// (extensions/i18n-default/i18n/*/project.ftl 의 「파일 트리」 절)에 있다. 여기 적은 id 가 ko · en 리소스에 모두 있는지는
// src/i18n/resources.test.ts 가 본다.

import { useLocalization } from "@fluent/react";
import { useMemo } from "react";

import { type Translate, translator } from "../i18n/localization";

export function fileTreeStrings(t: Translate) {
  return {
    label: t("file-tree-label"),
    noRoot: t("file-tree-no-root"),
    loading: t("file-tree-loading"),
    failed: (message: string) => t("file-tree-read-failed", { message }),
    outside: t("file-tree-outside"),
    hideIgnored: t("file-tree-ignored-hide"),
    showIgnored: t("file-tree-ignored-show"),
    search: t("file-tree-search-label"),
    searchNone: t("file-tree-search-none"),
    copyRelativePath: t("file-tree-copy-relative-path"),
    actions: {
      selectIn: t("action-file-tree-select-in"),
      toggleIgnored: t("action-file-tree-toggle-ignored"),
    },
    projectView: {
      label: t("open-with-project-view"),
      noFile: t("open-with-project-view-no-file"),
      outside: t("open-with-project-view-outside"),
    },
  };
}

export type FileTreeStrings = ReturnType<typeof fileTreeStrings>;

export function useFileTreeStrings(): FileTreeStrings {
  const { l10n } = useLocalization();
  return useMemo(() => fileTreeStrings(translator(l10n)), [l10n]);
}
