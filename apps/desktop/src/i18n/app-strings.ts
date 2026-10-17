// 모든 창이 함께 쓰는 문자열(액션 · 액션 찾기 · 테마와 언어 고르기 · 키맵 파일 오류 · 창 제목). 내용은 Fluent 리소스
// (extensions/i18n-default/i18n/*/app.ftl)에 있고, 이 파일은 화면 자리와 메시지 id 를 잇는다. 여기 적은 id 가 ko · en 리소스에
// 모두 있는지는 resources.test.ts 가 본다.

import { useLocalization } from "@fluent/react";
import { useMemo } from "react";

import type { ThemePreference } from "../theme/theme-store";
import { type Translate, translator } from "./localization";

export function appStrings(t: Translate) {
  return {
    windowTitles: {
      project: t("window-title-project"),
      settings: t("window-title-settings"),
    },
    actions: {
      find: t("action-find"),
      settings: t("action-settings"),
      cliInstall: t("action-cli-install"),
      theme: (theme: ThemePreference) => t("action-theme", { theme }),
      /** `name` 은 그 언어로 쓴 그 언어의 이름(`i18n.get` 의 `languages[].name`). */
      language: (name: string) => t("action-language", { name }),
      languageSystem: t("action-language-system"),
    },
    findAction: {
      label: t("find-action-label"),
      placeholder: t("find-action-placeholder"),
      empty: t("find-action-empty"),
      reserved: (reason: string) => t("find-action-reserved", { reason }),
    },
    theme: {
      label: t("theme-switcher-label"),
      system: t("theme-system"),
      light: t("theme-light"),
      dark: t("theme-dark"),
    },
    keymap: {
      error: (error: string) => t("keymap-file-error", { error }),
      warning: (warnings: string) => t("keymap-file-warning", { warnings }),
    },
    settings: {
      appearance: t("settings-appearance"),
      theme: t("settings-theme"),
      language: t("settings-language"),
      languageSystem: t("settings-language-system"),
      languageFailed: (error: string) => t("settings-language-failed", { error }),
    },
  };
}

export type AppStrings = ReturnType<typeof appStrings>;

/** 지금 언어의 공용 문자열. 언어가 바뀌면 새 객체다. */
export function useAppStrings(): AppStrings {
  const { l10n } = useLocalization();
  return useMemo(() => appStrings(translator(l10n)), [l10n]);
}
