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
      newFile: t("action-file-tree-new-file"),
      newFolder: t("action-file-tree-new-folder"),
      rename: t("action-file-tree-rename"),
      delete: t("action-file-tree-delete"),
      openInTerminal: t("action-file-tree-open-in-terminal"),
    },
    ops: {
      menu: {
        newFile: t("file-ops-menu-new-file"),
        newFolder: t("file-ops-menu-new-folder"),
        rename: t("file-ops-menu-rename"),
        delete: t("file-ops-menu-delete"),
        openInTerminal: t("file-ops-menu-open-in-terminal"),
      },
      name: {
        newFile: t("file-ops-name-new-file"),
        newFolder: t("file-ops-name-new-folder"),
        rename: t("file-ops-name-rename"),
        in: (folder: string) => t("file-ops-name-in", { folder }),
        label: t("file-ops-name-label"),
        empty: t("file-ops-name-empty"),
        separator: t("file-ops-name-separator"),
        dots: t("file-ops-name-dots"),
      },
      ok: t("file-ops-ok"),
      cancel: t("file-ops-cancel"),
      delete: {
        title: t("file-ops-delete-title"),
        body: (count: number, name: string) => t("file-ops-delete-body", { count, name }),
        /** `files`: 줄마다 이름 하나. */
        unsaved: (files: string) => t("file-ops-delete-unsaved", { files }),
        confirm: t("file-ops-delete-confirm"),
      },
      permanent: {
        title: t("file-ops-permanent-title"),
        body: (files: string, reason: string) => t("file-ops-permanent-body", { files, reason }),
        confirm: t("file-ops-permanent-confirm"),
      },
      exists: (name: string) => t("file-ops-exists", { name }),
      failed: {
        create: (message: string) => t("file-ops-create-failed", { message }),
        rename: (message: string) => t("file-ops-rename-failed", { message }),
        move: (message: string) => t("file-ops-move-failed", { message }),
        copy: (message: string) => t("file-ops-copy-failed", { message }),
        delete: (message: string) => t("file-ops-delete-failed", { message }),
      },
      deletePermission: (files: string) => t("file-ops-delete-permission", { files }),
      drop: {
        task: t("file-ops-drop-task"),
        conflicts: (files: string) => t("file-ops-drop-conflicts", { files }),
        skipped: (files: string) => t("file-ops-drop-skipped", { files }),
        cancelled: t("file-ops-drop-cancelled"),
        failed: (message: string) => t("file-ops-drop-failed", { message }),
      },
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
