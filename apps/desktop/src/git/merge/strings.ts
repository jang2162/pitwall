// 충돌 해결 화면(진행 중 작업 배너 · 충돌 목록 · 3-way 머지 문서, 13-m2-plan M2-19)이 보여 주는 문자열. 내용은 Fluent 리소스
// extensions/i18n-default/i18n/*/git-merge.ftl 에 있다. 여기 적은 id 가 ko · en 리소스에 모두 있는지는 src/i18n/resources.test.ts 가
// 본다.

import { useLocalization } from "@fluent/react";
import type { GitConflict, RepoOperation } from "@pitwall/rpc-client";
import { useMemo } from "react";

import { type Translate, translator } from "../../i18n/localization";

/** [병합…] 을 끈 까닭. */
export type PickOnlyReason = "binary" | "large" | "deleted" | "submodule" | "symlink" | "other";

export function gitMergeStrings(t: Translate) {
  const kind = (k: RepoOperation) => t("git-merge-kind", { kind: k });
  return {
    actions: {
      resolveConflicts: t("git-merge-action-resolve-conflicts"),
      open: t("git-merge-action-open"),
      continue: t("git-merge-action-continue"),
      skip: t("git-merge-action-skip"),
      abort: t("git-merge-action-abort"),
    },
    kind,
    banner: {
      label: t("git-merge-banner-label"),
      title: (k: RepoOperation) => t("git-merge-banner-title", { kind: kind(k) }),
      rebase: (branch: string, onto: string) => t("git-merge-banner-rebase", { branch, onto }),
      step: (current: number, total: number) => t("git-merge-banner-step", { current, total }),
      remaining: (count: number) => t("git-merge-banner-remaining", { count }),
      conflicts: (count: number) => t("git-merge-banner-conflicts", { count }),
      resolve: t("git-merge-banner-resolve"),
      continue: t("git-merge-banner-continue"),
      skip: t("git-merge-banner-skip"),
      abort: t("git-merge-banner-abort"),
    },
    abort: {
      title: (k: RepoOperation) => t("git-merge-abort-title", { kind: kind(k) }),
      body: t("git-merge-abort-body"),
      confirm: t("git-merge-abort-confirm"),
      unsaved: (count: number) => t("git-merge-abort-unsaved", { count }),
    },
    results: {
      done: (k: RepoOperation) => t("git-merge-result-done", { kind: kind(k) }),
      stopped: t("git-merge-result-stopped"),
      skipped: t("git-merge-result-skipped"),
      aborted: (k: RepoOperation) => t("git-merge-result-aborted", { kind: kind(k) }),
      failed: (action: string) => t("git-merge-result-failed", { action }),
    },
    conflicts: {
      title: t("git-merge-conflicts-title"),
      loading: t("git-merge-conflicts-loading"),
      empty: t("git-merge-conflicts-empty"),
      failed: (message: string) => t("git-merge-conflicts-failed", { message }),
      ours: t("git-merge-conflicts-ours"),
      theirs: t("git-merge-conflicts-theirs"),
      merge: t("git-merge-conflicts-merge"),
      close: t("git-merge-conflicts-close"),
      changed: t("git-merge-conflicts-changed"),
      kind: (k: GitConflict) => t("git-merge-conflict-kind", { kind: k }),
      pickOnly: (reason: PickOnlyReason) => t("git-merge-pick-only", { reason }),
    },
    doc: {
      title: (name: string) => t("git-merge-title", { name }),
      loading: t("git-merge-loading"),
      failed: (message: string) => t("git-merge-failed", { message }),
      gone: t("git-merge-gone"),
      ours: t("git-merge-pane-ours"),
      result: t("git-merge-pane-result"),
      theirs: t("git-merge-pane-theirs"),
      unresolved: (count: number) => t("git-merge-unresolved", { count }),
      clean: t("git-merge-clean"),
      applyNonConflicting: t("git-merge-apply-non-conflicting"),
      compareBase: t("git-merge-compare-base"),
      previous: t("git-merge-previous"),
      next: t("git-merge-next"),
      save: t("git-merge-save"),
      applyOurs: t("git-merge-apply-ours"),
      applyTheirs: t("git-merge-apply-theirs"),
      ignore: t("git-merge-ignore"),
      pickOurs: t("git-merge-pick-ours"),
      pickTheirs: t("git-merge-pick-theirs"),
      sideMissing: t("git-merge-side-missing"),
      unresolvedTitle: (count: number) => t("git-merge-save-unresolved-title", { count }),
      unresolvedBody: t("git-merge-save-unresolved-body"),
      unresolvedConfirm: t("git-merge-save-unresolved-confirm"),
      saved: (path: string) => t("git-merge-saved", { path }),
      saveConflict: t("git-merge-save-conflict"),
      allResolved: t("git-merge-all-resolved"),
    },
  };
}

export type GitMergeStrings = ReturnType<typeof gitMergeStrings>;

export function useGitMergeStrings(): GitMergeStrings {
  const { l10n } = useLocalization();
  return useMemo(() => gitMergeStrings(translator(l10n)), [l10n]);
}
