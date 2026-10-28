// 커밋 툴 윈도우(⌘0)와 diff 미리보기의 hunk · 줄 체크가 보여 주는 문자열. 내용은 Fluent 리소스
// extensions/i18n-default/i18n/*/git-commit.ftl 에 있다. 여기 적은 id 가 ko · en 리소스에 모두 있는지는 src/i18n/resources.test.ts 가
// 본다.

import { useLocalization } from "@fluent/react";
import { useMemo } from "react";

import { type Translate, translator } from "../../i18n/localization";

export function gitCommitStrings(t: Translate) {
  return {
    title: t("git-commit-title"),
    toolbar: {
      refresh: t("git-commit-refresh"),
      rollback: t("git-commit-rollback"),
      showDiff: t("git-commit-show-diff"),
    },
    groups: {
      conflicts: t("git-commit-group-conflicts"),
      staged: t("git-commit-group-staged"),
      changes: t("git-commit-group-changes"),
      untracked: t("git-commit-group-untracked"),
      stash: t("git-commit-group-stash"),
      count: (name: string, count: number) => t("git-commit-group-count", { name, count }),
    },
    list: {
      partial: t("git-commit-partial"),
      stage: t("git-commit-stage"),
      unstage: t("git-commit-unstage"),
      stageAll: t("git-commit-stage-all"),
      unstageAll: t("git-commit-unstage-all"),
      conflictHint: t("git-commit-conflict-hint"),
      noChanges: t("git-commit-no-changes"),
      noRepository: t("git-commit-no-repository"),
      unsupported: t("git-commit-unsupported"),
      noProject: t("git-commit-no-project"),
    },
    message: {
      label: t("git-commit-message"),
      history: t("git-commit-history"),
      historyEmpty: t("git-commit-history-empty"),
      amend: t("git-commit-amend"),
      signoff: t("git-commit-signoff"),
      commit: t("git-commit-run"),
      commitAndPush: t("git-commit-run-and-push"),
      running: t("git-commit-running"),
      nothingStaged: t("git-commit-nothing-staged"),
      empty: t("git-commit-empty-message"),
    },
    result: {
      output: t("git-commit-output"),
      outputFailed: t("git-commit-output-failed"),
      close: t("git-commit-output-close"),
      done: (hash: string) => t("git-commit-done", { hash }),
      amended: (previous: string, hash: string) => t("git-commit-amended", { previous, hash }),
      pushUnavailable: t("git-commit-push-unavailable"),
      failed: (message: string) => t("git-commit-failed", { message }),
    },
    rollback: {
      title: t("git-commit-rollback-title"),
      body: (count: number) => t("git-commit-rollback-body", { count }),
      confirm: t("git-commit-rollback-confirm"),
      untracked: (files: string) => t("git-commit-rollback-untracked", { files }),
      none: t("git-commit-rollback-none"),
      more: (count: number) => t("git-commit-more", { count }),
    },
    hunk: {
      stage: t("git-commit-hunk-stage"),
      unstage: t("git-commit-hunk-unstage"),
      stageLine: t("git-commit-line-stage"),
      unstageLine: t("git-commit-line-unstage"),
      revert: t("git-commit-hunk-revert"),
      revertTitle: t("git-commit-hunk-revert-title"),
      revertBody: t("git-commit-hunk-revert-body"),
      changed: t("git-commit-hunk-changed"),
    },
    stash: {
      create: t("git-commit-stash-create"),
      includeUntracked: t("git-commit-stash-include-untracked"),
      name: t("git-commit-stash-name"),
      hint: t("git-commit-stash-hint"),
      apply: t("git-commit-stash-apply"),
      pop: t("git-commit-stash-pop"),
      drop: t("git-commit-stash-drop"),
      dropTitle: t("git-commit-stash-drop-title"),
      dropBody: (name: string) => t("git-commit-stash-drop-body", { name }),
      nothing: t("git-commit-stash-nothing"),
      conflicted: t("git-commit-stash-conflicted"),
      changed: t("git-commit-stash-changed"),
      empty: t("git-commit-stash-empty"),
    },
    actions: {
      open: t("git-commit-action-open"),
      run: t("git-commit-action-run"),
      runAndPush: t("git-commit-action-run-and-push"),
      rollback: t("git-commit-action-rollback"),
    },
  };
}

export type GitCommitStrings = ReturnType<typeof gitCommitStrings>;

export function useGitCommitStrings(): GitCommitStrings {
  const { l10n } = useLocalization();
  return useMemo(() => gitCommitStrings(translator(l10n)), [l10n]);
}
