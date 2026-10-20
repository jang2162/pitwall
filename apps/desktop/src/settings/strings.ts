// 설정 창의 「외부 도구 > Orca 에서 Pitwall 열기」 가 보여 주는 문자열. 내용은 Fluent 리소스(extensions/i18n-default/i18n/*/app.ftl 의
// `settings-orca-*`)에 있고, 이 파일은 화면 자리와 메시지 id 를 잇는다. 여기 적은 id 가 ko · en 리소스에 모두 있는지는
// src/i18n/resources.test.ts 가 본다. 외관(테마 · 언어)의 문자열은 i18n/app-strings.ts 의 `settings` 에 있다.

import { useLocalization } from "@fluent/react";
import { useMemo } from "react";

import { type Translate, translator } from "../i18n/localization";

export function settingsStrings(t: Translate) {
  return {
    externalTools: t("settings-external-tools"),
    orca: {
      title: t("settings-orca-title"),
      intro: t("settings-orca-intro"),
      label: t("settings-orca-label"),
      command: t("settings-orca-command"),
      copy: t("settings-orca-copy"),
      copyValue: (value: string) => t("settings-orca-copy-value", { value }),
      copied: t("settings-orca-copied"),
      copyFailed: (error: string) => t("settings-orca-copy-failed", { error }),
      checking: t("settings-orca-checking"),
      checkFailed: (error: string) => t("settings-orca-check-failed", { error }),
      recheck: t("settings-orca-recheck"),
      install: {
        thisApp: t("settings-orca-installed"),
        missing: t("settings-orca-not-installed"),
        otherLink: t("settings-orca-other-link"),
        notALink: t("settings-orca-not-a-link"),
      },
      onPath: {
        thisApp: t("settings-orca-on-path"),
        notFound: t("settings-orca-not-on-path"),
        other: t("settings-orca-other-on-path"),
      },
      detail: {
        installed: (target: string) => t("settings-orca-detail-installed", { target }),
        missing: (target: string) => t("settings-orca-detail-missing", { target }),
        otherLink: (target: string, points: string) =>
          t("settings-orca-detail-other-link", { target, points }),
        notALink: (target: string) => t("settings-orca-detail-not-a-link", { target }),
        otherOnPath: (path: string) => t("settings-orca-detail-other-on-path", { path }),
        resolved: (resolved: string) => t("settings-orca-detail-resolved", { resolved }),
      },
      installButton: t("settings-orca-install"),
      installing: t("settings-orca-installing"),
      absolute: (target: string) => t("settings-orca-absolute", { target }),
      pathFallback: (reason: string) => t("settings-orca-path-fallback", { reason }),
      worktree: t("settings-orca-worktree"),
    },
  };
}

export type SettingsStrings = ReturnType<typeof settingsStrings>;

/** 지금 언어의 설정 창 문자열. 언어가 바뀌면 새 객체다. */
export function useSettingsStrings(): SettingsStrings {
  const { l10n } = useLocalization();
  return useMemo(() => settingsStrings(translator(l10n)), [l10n]);
}
