// 파일 히스토리(⌃⌥H)가 보여 주는 문자열. 내용은 Fluent 리소스 extensions/i18n-default/i18n/*/git-history.ftl 에 있다. 여기 적은
// id 가 ko · en 리소스에 모두 있는지는 src/i18n/resources.test.ts 가 본다.

import { useLocalization } from "@fluent/react";
import type { FileChangeKind } from "@pitwall/rpc-client";
import { useMemo } from "react";

import { type Translate, translator } from "../../i18n/localization";

export function gitHistoryStrings(t: Translate) {
  return {
    actions: { show: t("git-history-action-show") },
    menu: t("git-history-menu"),
    title: (name: string) => t("git-history-title", { name }),
    list: t("git-history-list"),
    loading: t("git-history-loading"),
    empty: t("git-history-empty"),
    failed: (message: string) => t("git-history-failed", { message }),
    count: (count: number) => t("git-history-count", { count }),
    pick: t("git-history-pick"),
    renamed: (from: string, to: string) => t("git-history-renamed", { from, to }),
    copied: (from: string, to: string) => t("git-history-copied", { from, to }),
    pathAt: (path: string) => t("git-history-path-at", { path }),
    change: {
      added: t("git-history-change-added"),
      modified: t("git-history-change-modified"),
      deleted: t("git-history-change-deleted"),
      renamed: t("git-history-change-renamed"),
      copied: t("git-history-change-copied"),
      typeChanged: t("git-history-change-type-changed"),
    } satisfies Record<FileChangeKind, string>,
    rowMenu: {
      reveal: t("git-history-menu-reveal"),
      showCommit: t("git-history-menu-show-commit"),
      openDiff: t("git-history-menu-open-diff"),
      openRevision: t("git-history-menu-open-revision"),
      copyHash: t("git-history-menu-copy-hash"),
    },
    copiedHash: t("git-history-copied-hash"),
    open: {
      noFile: t("git-history-no-file"),
      noFileBody: t("git-history-no-file-body"),
      outside: t("git-history-outside"),
      folder: t("git-history-folder"),
      failed: (message: string) => t("git-history-open-failed", { message }),
    },
  };
}

export type GitHistoryStrings = ReturnType<typeof gitHistoryStrings>;

export function useGitHistoryStrings(): GitHistoryStrings {
  const { l10n } = useLocalization();
  return useMemo(() => gitHistoryStrings(translator(l10n)), [l10n]);
}
