// 상태바 브랜치 항목 · 브랜치 팝업(⌃⇧B) · VCS 작업 팝업(⌃V) · Search Everywhere 의 브랜치 탭이 보여 주는 문자열(13-m2-plan.md M2-13).
// 내용은 Fluent 리소스(extensions/i18n-default/i18n/*/project.ftl 의 「브랜치」 절)에 있다. 여기 적은 id 가 ko · en 리소스에 모두
// 있는지는 src/i18n/resources.test.ts 가 본다.

import { useLocalization } from "@fluent/react";
import type { RepoOperation } from "@pitwall/rpc-client";
import { useMemo } from "react";

import { type Translate, translator } from "../i18n/localization";

export function branchStrings(t: Translate) {
  return {
    actions: {
      branches: t("action-git-branches"),
      vcsOperations: t("action-git-vcs-operations"),
    },
    status: {
      /** 상태바 항목의 툴팁: 잡은 저장소(작업 트리 꼭대기). */
      repository: (path: string) => t("branch-status-repository", { path }),
      /** 상태바 항목의 스크린 리더 이름. */
      label: (name: string) => t("branch-status-label", { name }),
      unsupported: t("branch-status-unsupported"),
      unsupportedDetail: t("branch-status-unsupported-detail"),
      detached: t("branch-status-detached"),
      operation: (kind: RepoOperation) => t("branch-status-operation", { kind }),
    },
    popup: {
      label: t("branch-popup-label"),
      placeholder: t("branch-popup-placeholder"),
      empty: t("branch-popup-empty"),
      loading: t("branch-popup-loading"),
      noRepository: t("branch-popup-no-repository"),
      failed: (message: string) => t("branch-popup-failed", { message }),
      groups: {
        favorites: t("branch-group-favorites"),
        local: t("branch-group-local"),
        remote: t("branch-group-remote"),
        tags: t("branch-group-tags"),
        worktrees: t("branch-group-worktrees"),
      },
      newBranch: t("branch-new"),
      current: t("branch-current"),
      favorite: t("branch-favorite"),
      unfavorite: t("branch-unfavorite"),
      worktreeMain: t("branch-worktree-main"),
      worktreeCurrent: t("branch-worktree-current"),
      worktreeMissing: t("branch-worktree-missing"),
      worktreeLocked: t("branch-worktree-locked"),
      worktreeOther: t("branch-worktree-other"),
      back: t("branch-back"),
      /** 브랜치 하나를 고른 뒤의 동작 목록. */
      menu: {
        checkout: t("branch-checkout"),
        checkoutTracking: (local: string) => t("branch-checkout-tracking", { local }),
        newFrom: (name: string) => t("branch-new-from", { name }),
        merge: (name: string, current: string) => t("branch-merge", { name, current }),
        rebase: (current: string, name: string) => t("branch-rebase", { current, name }),
        compare: (name: string, current: string) => t("branch-compare", { name, current }),
        rename: t("branch-rename"),
        delete: t("branch-delete"),
      },
    },
    dialogs: {
      newTitle: t("branch-new-title"),
      newHint: (start: string) => t("branch-new-hint", { start }),
      renameTitle: (name: string) => t("branch-rename-title", { name }),
      emptyName: t("branch-name-empty"),
      badName: t("branch-name-bad"),
      forceTitle: (name: string) => t("branch-force-delete-title", { name }),
      forceBody: (name: string) => t("branch-force-delete-body", { name }),
      forceConfirm: t("branch-force-delete-confirm"),
      mergeTitle: (name: string, current: string) => t("branch-merge-title", { name, current }),
      rebaseTitle: (current: string, name: string) => t("branch-rebase-title", { current, name }),
      historyBody: t("branch-history-body"),
      proceed: t("branch-proceed"),
    },
    results: {
      checkedOut: (name: string) => t("branch-result-checked-out", { name }),
      created: (name: string) => t("branch-result-created", { name }),
      renamed: (name: string) => t("branch-result-renamed", { name }),
      deleted: (name: string, commit: string) => t("branch-result-deleted", { name, commit }),
      merged: (name: string) => t("branch-result-merged", { name }),
      rebased: (name: string) => t("branch-result-rebased", { name }),
      upToDate: t("branch-result-up-to-date"),
      stopped: t("branch-result-stopped"),
      failed: (what: string) => t("branch-result-failed", { what }),
      compareMissing: t("branch-compare-missing"),
    },
    vcs: {
      label: t("vcs-popup-label"),
      placeholder: t("vcs-popup-placeholder"),
      empty: t("vcs-popup-empty"),
    },
  };
}

export type BranchStrings = ReturnType<typeof branchStrings>;

export function useBranchStrings(): BranchStrings {
  const { l10n } = useLocalization();
  return useMemo(() => branchStrings(translator(l10n)), [l10n]);
}
