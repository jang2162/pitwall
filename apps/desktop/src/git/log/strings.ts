// Git 로그 툴 윈도우(⌘9)가 보여 주는 문자열. 내용은 Fluent 리소스 extensions/i18n-default/i18n/*/git-log.ftl 에 있다. 여기 적은 id
// 가 ko · en 리소스에 모두 있는지는 src/i18n/resources.test.ts 가 본다.

import { useLocalization } from "@fluent/react";
import { useMemo } from "react";

import { type Translate, translator } from "../../i18n/localization";

export function gitLogStrings(t: Translate) {
  return {
    list: t("git-log-list"),
    branches: {
      title: t("git-log-branches"),
      head: t("git-log-branches-head"),
      local: t("git-log-branches-local"),
      remote: t("git-log-branches-remote"),
      tags: t("git-log-branches-tags"),
      all: t("git-log-branches-all"),
    },
    filter: {
      text: t("git-log-filter-text"),
      regex: t("git-log-filter-regex"),
      matchCase: t("git-log-filter-match-case"),
      branch: t("git-log-filter-branch"),
      branchCount: (count: number) => t("git-log-filter-branch-count", { count }),
      author: t("git-log-filter-author"),
      date: t("git-log-filter-date"),
      since: t("git-log-filter-since"),
      until: t("git-log-filter-until"),
      path: t("git-log-filter-path"),
      pathHint: t("git-log-filter-path-hint"),
      clear: t("git-log-filter-clear"),
      noGraph: t("git-log-filter-no-graph"),
    },
    status: {
      loading: t("git-log-loading"),
      count: (count: number) => t("git-log-count", { count }),
      matched: (count: number) => t("git-log-matched", { count }),
      matchedHash: (count: number) => t("git-log-matched-hash", { count }),
      failed: (message: string) => t("git-log-failed", { message }),
      empty: t("git-log-empty"),
      noProject: t("git-log-no-project"),
    },
    detail: {
      none: t("git-log-detail-none"),
      files: (count: number) => t("git-log-detail-files", { count }),
      filesLoading: t("git-log-detail-files-loading"),
      parents: t("git-log-detail-parents"),
      root: t("git-log-detail-root"),
      author: t("git-log-detail-author"),
      committer: t("git-log-detail-committer"),
      containedIn: t("git-log-detail-contained-in"),
      containedLoading: t("git-log-detail-contained-loading"),
      containedNone: t("git-log-detail-contained-none"),
      generated: t("git-log-detail-generated"),
      compare: (a: string, b: string) => t("git-log-detail-compare", { a, b }),
      openCompare: t("git-log-detail-open-compare"),
      openCommit: t("git-log-detail-open-commit"),
      many: (count: number) => t("git-log-detail-many", { count }),
      failed: (message: string) => t("git-log-detail-failed", { message }),
    },
    menu: {
      checkoutBranch: (name: string) => t("git-log-menu-checkout-branch", { name }),
      checkoutRevision: t("git-log-menu-checkout-revision"),
      newBranch: t("git-log-menu-new-branch"),
      newTag: t("git-log-menu-new-tag"),
      cherryPick: (count: number) => t("git-log-menu-cherry-pick", { count }),
      revert: (count: number) => t("git-log-menu-revert", { count }),
      reset: t("git-log-menu-reset"),
      resetMode: (mode: string) => t("git-log-menu-reset-mode", { mode }),
      copyHash: t("git-log-menu-copy-hash"),
      showCommit: t("git-log-menu-show-commit"),
      compare: t("git-log-menu-compare"),
      merge: (source: string, target: string) => t("git-log-menu-merge", { source, target }),
      mergeNeedsCheckout: (target: string) => t("git-log-menu-merge-needs-checkout", { target }),
      rebase: (source: string, target: string) => t("git-log-menu-rebase", { source, target }),
      compareRefs: (a: string, b: string) => t("git-log-menu-compare-refs", { a, b }),
    },
    dialog: {
      newTag: t("git-log-dialog-new-tag"),
      from: (rev: string) => t("git-log-dialog-from", { rev }),
      nameEmpty: t("git-log-dialog-name-empty"),
      nameInvalid: t("git-log-dialog-name-invalid"),
    },
    result: {
      moved: (ref: string, from: string, to: string) =>
        t("git-log-result-moved", { ref, from, to }),
      undo: t("git-log-result-undo"),
      dismiss: t("git-log-result-dismiss"),
      stopped: t("git-log-result-stopped"),
      stoppedBody: t("git-log-result-stopped-body"),
      upToDate: t("git-log-result-up-to-date"),
      failed: (action: string) => t("git-log-result-failed", { action }),
      copied: t("git-log-result-copied"),
    },
  };
}

export type GitLogStrings = ReturnType<typeof gitLogStrings>;

export function useGitLogStrings(): GitLogStrings {
  const { l10n } = useLocalization();
  return useMemo(() => gitLogStrings(translator(l10n)), [l10n]);
}
