// 웰컴 창(S1 · S2 · S3)이 보여 주는 문자열. 내용은 Fluent 리소스(extensions/i18n-default/i18n/*/welcome.ftl)에 있고, 이 파일은
// 화면 자리와 메시지 id 를 잇는다 — 화면 코드에는 문자열을 두지 않고 여기서만 가져다 쓴다. 값이 끼는 문자열은 함수다(Fluent 의
// 변수 자리). 여기 적은 id 가 ko · en 리소스에 모두 있는지는 src/i18n/resources.test.ts 가 본다.

import type { Platform } from "@pitwall/keymap";
import { useLocalization } from "@fluent/react";
import { useMemo } from "react";

import { type Translate, translator } from "../i18n/localization";

export function welcomeStrings(t: Translate) {
  const error = {
    action: (what: string, message: string) => t("welcome-error-action", { what, message }),
    open: t("welcome-error-open"),
    remove: t("welcome-error-remove"),
    pickFolder: t("welcome-error-pick-folder"),
    reveal: t("welcome-error-reveal"),
    create: t("welcome-error-create"),
    findProject: t("welcome-error-find-project"),
    candidates: t("welcome-error-candidates"),
  };
  return {
    nav: {
      label: t("welcome-nav-label"),
      projects: t("welcome-nav-projects"),
      remoteHosts: t("welcome-nav-remote-hosts"),
      extensions: t("welcome-nav-extensions"),
      /** 아직 없는 탭에 붙는 설명. */
      notYet: t("welcome-nav-not-yet"),
    },
    list: {
      label: t("welcome-list-label"),
      search: t("welcome-list-search"),
      empty: t("welcome-list-empty"),
      noMatch: t("welcome-list-no-match"),
      favorite: t("welcome-list-favorite"),
      remote: t("welcome-list-remote"),
    },
    buttons: {
      newProject: t("welcome-button-new-project"),
      openFolder: t("welcome-button-open-folder"),
    },
    menu: {
      open: t("welcome-menu-open"),
      reveal: (platform: Platform) => t("welcome-menu-reveal", { platform }),
      remove: t("welcome-menu-remove"),
    },
    actions: {
      newProject: t("welcome-action-new-project"),
      openFolder: t("welcome-action-open-folder"),
      open: t("welcome-action-open"),
      reveal: (platform: Platform) => t("welcome-action-reveal", { platform }),
      remove: t("welcome-action-remove"),
      create: t("welcome-action-create"),
    },
    drop: t("welcome-drop"),
    storage: {
      recovered: (reason: string, backup: string) =>
        t("welcome-storage-recovered", { reason, backup }),
      readOnly: (reason: string) => t("welcome-storage-read-only", { reason }),
      memory: (reason: string) => t("welcome-storage-memory", { reason }),
    },
    find: {
      label: t("welcome-find-label"),
      title: (path: string) => t("welcome-find-title", { path }),
      search: t("welcome-find-search"),
      empty: t("welcome-find-empty"),
      loading: t("welcome-find-loading"),
      recommended: t("welcome-find-recommended"),
      worktreeMain: t("welcome-find-worktree-main"),
      pickFolder: t("welcome-find-pick-folder"),
      fileOnly: t("welcome-find-file-only"),
      fileOnlyNotYet: t("welcome-find-file-only-not-yet"),
    },
    create: {
      title: (name: string) => t("welcome-create-title", { name }),
      name: t("welcome-create-name"),
      root: t("welcome-create-root"),
      changeRoot: t("welcome-create-change-root"),
      chooseRoot: t("welcome-create-choose-root"),
      rootNone: t("welcome-create-root-none"),
      configMode: t("welcome-create-config-mode"),
      repository: t("welcome-create-repository"),
      personal: t("welcome-create-personal"),
      detected: t("welcome-create-detected"),
      detectedEmpty: t("welcome-create-detected-empty"),
      cancel: t("welcome-create-cancel"),
      submit: t("welcome-create-submit"),
    },
    notices: {
      fileNotInProject: (path: string) => t("welcome-notice-file-not-in-project", { path }),
    },
    errors: error,
  };
}

export type WelcomeStrings = ReturnType<typeof welcomeStrings>;

/** 지금 언어의 웰컴 창 문자열. 언어가 바뀌면 새 객체다. */
export function useWelcomeStrings(): WelcomeStrings {
  const { l10n } = useLocalization();
  return useMemo(() => welcomeStrings(translator(l10n)), [l10n]);
}
