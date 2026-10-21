// 모든 창이 함께 쓰는 문자열(액션 · 액션 찾기 · Search Everywhere · 테마와 언어 고르기 · 키맵 파일 오류 · 창 제목 · 다음에서 열기). 설정 창에만 나오는 것은
// settings/strings.ts 에 있다. 내용은 Fluent 리소스(extensions/i18n-default/i18n/*/app.ftl)에 있고, 이 파일은 화면 자리와 메시지 id
// 를 잇는다. 여기 적은 id 가 ko · en 리소스에 모두 있는지는 resources.test.ts 가 본다.

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
      searchEverywhere: t("action-search-everywhere"),
      gotoFile: t("action-goto-file"),
      recentFiles: t("action-recent-files"),
      projectSwitcher: t("action-project-switcher"),
      settings: t("action-settings"),
      cliInstall: t("action-cli-install"),
      closeWindow: t("action-window-close"),
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
    search: {
      label: t("search-everywhere-label"),
      recentFiles: t("search-recent-files-label"),
      tabs: {
        all: t("search-tab-all"),
        files: t("search-tab-files"),
        actions: t("search-tab-actions"),
        projects: t("search-tab-projects"),
      },
      placeholder: {
        all: t("search-placeholder-all"),
        files: t("search-placeholder-files"),
        projects: t("search-placeholder-projects"),
        recentFiles: t("search-placeholder-recent-files"),
      },
      empty: t("search-empty"),
      more: t("search-more"),
      noRecentFiles: t("search-no-recent-files"),
      searching: t("search-files-searching"),
      busy: t("search-files-busy"),
      failed: (message: string) => t("search-failed", { message }),
      /** 상한을 넘어 일부만 보일 때. */
      truncated: (shown: number, matched: number) =>
        t("search-files-truncated", { shown, matched }),
      /** `파일:줄` 로 칠 때 결과 옆의 「줄 N」. */
      atLine: (line: number) => t("search-files-at-line", { line }),
      recentBadge: t("search-recent-badge"),
      currentProject: t("search-current-project"),
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
    openWith: {
      action: t("action-open-with"),
      actionDefault: t("action-open-with-default"),
      label: t("open-with-label"),
      title: (project: string) => t("open-with-title", { project }),
      placeholder: t("open-with-placeholder"),
      empty: t("open-with-empty"),
      loading: t("open-with-loading"),
      defaultBadge: t("open-with-default-badge"),
      unavailable: (reason: string) => t("open-with-unavailable", { reason }),
      noDefault: t("open-with-no-default"),
      unknownDefault: (tool: string) => t("open-with-unknown-default", { tool }),
      failed: (tool: string, reason: string) => t("open-with-failed", { tool, reason }),
      command: (command: string) => t("open-with-command", { command }),
      dismiss: t("open-with-dismiss"),
      /** 도구를 고르지 않고(⌘⌥E) 실행이 오류로 끝났을 때 도구 이름 자리. */
      defaultTool: t("open-with-default-tool"),
      listFailed: (error: string) => t("open-with-list-failed", { error }),
      problem: {
        unknownTool: t("open-with-problem-unknown-tool"),
        invalidTool: (message: string) => t("open-with-problem-invalid-tool", { message }),
        missingVariable: (variable: string) =>
          t("open-with-problem-missing-variable", { variable }),
        projectRootMissing: (path: string) => t("open-with-problem-project-root-missing", { path }),
        /** `source`: 찾아본 PATH 가 로그인 셸의 것(`login`)인가 앱 프로세스의 것(`process`)인가. */
        notFound: (program: string, source: "login" | "process") =>
          t("open-with-problem-not-found", { program, source }),
        bundlePath: (path: string) => t("open-with-problem-bundle-path", { path }),
        spawnFailed: (program: string, message: string) =>
          t("open-with-problem-spawn-failed", { program, message }),
        /** `code`: 종료 코드, 신호로 끝났으면 `"signal"`. */
        exited: (code: string) => t("open-with-problem-exited", { code }),
      },
    },
  };
}

export type AppStrings = ReturnType<typeof appStrings>;

/** 지금 언어의 공용 문자열. 언어가 바뀌면 새 객체다. */
export function useAppStrings(): AppStrings {
  const { l10n } = useLocalization();
  return useMemo(() => appStrings(translator(l10n)), [l10n]);
}
