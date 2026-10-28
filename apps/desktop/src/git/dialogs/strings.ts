// 위험 작업 다이얼로그 · 푸시 · 풀 · fetch · 브랜치 이름 · [되돌리기] · 설정 창 「Git」 의 문자열(13-m2-plan.md M2-20). 내용은 Fluent
// 리소스 extensions/i18n-default/i18n/*/git-dialogs.ftl 에 있다. 여기 적은 id 가 ko · en 리소스에 모두 있는지는
// src/i18n/resources.test.ts 가 본다.

import { useLocalization } from "@fluent/react";
import type { GitPushRefusalReason, GitPushTargetSource } from "@pitwall/rpc-client";
import { useMemo } from "react";

import { type Translate, translator } from "../../i18n/localization";

export function gitDialogStrings(t: Translate) {
  return {
    actions: {
      push: t("action-git-push"),
      pull: t("action-git-pull"),
      fetch: t("action-git-fetch"),
    },
    common: {
      cancel: t("git-dialog-cancel"),
      danger: t("git-dialog-danger"),
      noProject: t("git-dialog-no-project"),
      noRepository: t("git-dialog-no-repository"),
      failed: (what: string) => t("git-dialog-failed", { what }),
    },
    lost: {
      count: (count: number) => t("git-lost-count", { count }),
      more: (count: number) => t("git-lost-more", { count }),
      nothing: t("git-lost-nothing"),
      unverifiable: t("git-lost-unverifiable"),
      changed: t("git-lost-changed"),
    },
    branch: {
      name: t("git-branch-name"),
      createTitle: t("git-branch-create-title"),
      createFrom: (from: string) => t("git-branch-create-from", { from }),
      checkout: t("git-branch-create-checkout"),
      create: t("git-branch-create"),
      renameTitle: (name: string) => t("git-branch-rename-title", { name }),
      rename: t("git-branch-rename"),
      empty: t("git-branch-name-empty"),
      invalid: t("git-branch-name-invalid"),
      taken: t("git-branch-name-taken"),
      forceTitle: (name: string) => t("git-branch-force-title", { name }),
      forceBody: (name: string, commit: string) => t("git-branch-force-body", { name, commit }),
      forceConfirm: t("git-branch-force-confirm"),
      deleted: (name: string, commit: string) => t("git-branch-deleted", { name, commit }),
    },
    undo: {
      button: t("git-undo"),
      moved: (ref: string, from: string, to: string) => t("git-undo-moved", { ref, from, to }),
      title: (commit: string) => t("git-undo-title", { commit }),
      confirm: t("git-undo-confirm"),
      done: (ref: string) => t("git-undo-done", { ref }),
      restored: (name: string) => t("git-undo-restored", { name }),
      removed: (name: string) => t("git-undo-removed", { name }),
      failed: t("git-undo-failed"),
    },
    reset: {
      title: (mode: string, commit: string) => t("git-reset-title", { mode, commit }),
      confirm: t("git-reset-confirm"),
    },
    push: {
      title: t("git-push-title"),
      loading: t("git-push-loading"),
      target: (branch: string, remote: string, remoteRef: string) =>
        t("git-push-target", { branch, remote, remoteRef }),
      source: (source: GitPushTargetSource) => t("git-push-source", { source }),
      terminal: t("git-push-terminal"),
      terminalDefault: (value: string) => t("git-push-terminal-default", { value }),
      terminalConfigured: (refspecs: string) => t("git-push-terminal-configured", { refspecs }),
      terminalSends: (sends: string) => t("git-push-terminal-sends", { sends }),
      terminalNothing: t("git-push-terminal-nothing"),
      terminalRefuses: (message: string) => t("git-push-terminal-refuses", { message }),
      refused: (reason: GitPushRefusalReason) => t("git-push-refused", { reason }),
      candidates: t("git-push-candidates"),
      commits: (count: number) => t("git-push-commits", { count }),
      commitsMore: t("git-push-commits-more"),
      noCommits: t("git-push-no-commits"),
      setUpstream: t("git-push-set-upstream"),
      force: t("git-push-force"),
      forceUnavailable: t("git-push-force-unavailable"),
      forceTitle: t("git-push-force-title"),
      forceBody: (remoteRef: string, commit: string) =>
        t("git-push-force-body", { remoteRef, commit }),
      forceConfirm: t("git-push-force-confirm"),
      send: t("git-push-send"),
      sending: t("git-push-sending"),
      changed: t("git-push-changed"),
      rejected: t("git-push-rejected"),
      pull: t("git-push-pull"),
      done: (branch: string, remote: string, remoteRef: string) =>
        t("git-push-done", { branch, remote, remoteRef }),
      doneRefspec: (refspec: string) => t("git-push-done-refspec", { refspec }),
    },
    pull: {
      title: t("git-pull-title"),
      mode: t("git-pull-mode"),
      merge: t("git-pull-merge"),
      rebase: t("git-pull-rebase"),
      remote: t("git-pull-remote"),
      branch: t("git-pull-branch"),
      upstream: (name: string) => t("git-pull-upstream", { name }),
      noUpstream: t("git-pull-no-upstream"),
      send: t("git-pull-send"),
      done: t("git-pull-done"),
      stopped: t("git-pull-stopped"),
    },
    fetch: {
      done: t("git-fetch-done"),
    },
    settings: {
      title: t("settings-git-title"),
      path: t("settings-git-path"),
      configured: (value: string) => t("settings-git-configured", { value }),
      notConfigured: t("settings-git-not-configured"),
      version: t("settings-git-version"),
      minimum: (minimum: string) => t("settings-git-minimum", { minimum }),
      writes: t("settings-git-writes"),
      writesOn: t("settings-git-writes-on"),
      notFound: (program: string) => t("settings-git-not-found", { program }),
      tooOld: (version: string, minimum: string) => t("settings-git-too-old", { version, minimum }),
      unreadable: (reason: string) => t("settings-git-unreadable", { reason }),
      shellPath: t("settings-git-shell-path"),
      processPath: (reason: string) => t("settings-git-process-path", { reason }),
      checking: t("settings-git-checking"),
      failed: (error: string) => t("settings-git-failed", { error }),
      recheck: t("settings-git-recheck"),
    },
  };
}

export type GitDialogStrings = ReturnType<typeof gitDialogStrings>;

/** 지금 언어의 문자열. 언어가 바뀌면 새 객체다. */
export function useGitDialogStrings(): GitDialogStrings {
  const { l10n } = useLocalization();
  return useMemo(() => gitDialogStrings(translator(l10n)), [l10n]);
}
