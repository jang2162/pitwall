// 커밋 보내기 · 가져오기의 문자열(18-m7-plan M7-17). 내용은 Fluent 리소스 extensions/i18n-default/i18n/*/git-send.ftl 에 있다. 여기 적은
// id 가 ko · en 리소스에 모두 있는지는 src/i18n/resources.test.ts 가 본다.

import { useLocalization } from "@fluent/react";
import type { GitSendStop } from "@pitwall/rpc-client";
import { useMemo } from "react";

import { type Translate, translator } from "../../i18n/localization";

export function gitSendStrings(t: Translate) {
  return {
    actions: {
      send: t("action-git-send"),
      receive: t("action-git-receive"),
    },
    pick: {
      send: t("git-send-pick-send"),
      receive: t("git-send-pick-receive"),
      placeholder: t("git-send-pick-placeholder"),
      empty: t("git-send-pick-empty"),
      main: t("git-send-main"),
      last: t("git-send-last"),
    },
    confirm: {
      title: (branch: string, from: string, to: string) =>
        t("git-send-confirm-title", { branch, from, to }),
      from: t("git-send-from"),
      to: t("git-send-to"),
      loading: t("git-send-loading"),
      notBranch: t("git-send-not-branch"),
      notConnected: t("git-send-not-connected"),
      uncommitted: t("git-send-uncommitted"),
      busy: (runs: number, terminals: number) => t("git-send-target-busy", { runs, terminals }),
      noForce: t("git-send-no-force"),
      send: t("git-send-go"),
      receive: t("git-send-go-receive"),
    },
    result: {
      progress: (to: string) => t("git-send-progress", { to }),
      cancel: t("git-send-cancel"),
      created: (branch: string, to: string) => t("git-send-created", { branch, to }),
      /** `range` 는 「이전 해시 → 지금 해시」. */
      moved: (branch: string, to: string, range: string) =>
        t("git-send-moved", { branch, to, range }),
      same: (branch: string, to: string) => t("git-send-same", { branch, to }),
      uncommitted: t("git-send-uncommitted-left"),
      lfs: (count: number) => t("git-send-lfs", { count }),
      runThere: t("git-send-run-there"),
      terminalThere: t("git-send-terminal-there"),
      stopped: (branch: string, to: string) => t("git-send-stopped", { branch, to }),
      incoming: (ref: string) => t("git-send-incoming", { ref }),
      reason: (reason: GitSendStop) => t("git-send-reason", { reason }),
      failed: t("git-send-failed"),
    },
  };
}

export type GitSendStrings = ReturnType<typeof gitSendStrings>;

export function useGitSendStrings(): GitSendStrings {
  const { l10n } = useLocalization();
  return useMemo(() => gitSendStrings(translator(l10n)), [l10n]);
}
