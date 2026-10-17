// 지금 등록하는 액션(브리프 M0-13: 이미 있는 기능만). 기능이 없는 액션(Git · DB 등)은 그 기능과 함께 등록한다.
// 기본 키는 extensions/keymap-default/keymaps/default.toml.

import type { Action } from "@pitwall/keymap";
import type { DockviewApi } from "dockview-react";

import type { AppStrings } from "../i18n/app-strings";
import { collapseAllDocks, PANELS, toggleToolWindow } from "../layout/project-layout";
import type { ThemePreference } from "../theme/theme-store";

export function appActions(deps: {
  strings: AppStrings;
  /** 등록된 UI 언어(그 언어로 쓴 이름). 언어마다 액션 하나와 「시스템」 액션을 둔다. */
  languages: readonly { locale: string; name: string }[];
  setLanguage(language: string): Promise<void>;
  openFindAction(): void;
  openSettings(): Promise<void>;
  setTheme(preference: ThemePreference): void;
  /** 없으면(macOS 가 아니면) 액션을 등록하지 않는다. */
  installShellCommand?: () => Promise<void>;
}): Action[] {
  const { installShellCommand } = deps;
  const T = deps.strings.actions;
  // 실패(설정 파일을 고칠 수 없음 등)는 RPC 오류로 온다. 설정 창은 화면에 보이고, 액션은 로그로 남긴다.
  const setLanguage = (language: string) => () =>
    deps
      .setLanguage(language)
      .catch((e: unknown) => console.warn("[i18n] language not changed", language, e));
  return [
    {
      id: "action.find",
      label: T.find,
      keywords: ["Find Action", "command palette"],
      run: deps.openFindAction,
    },
    {
      id: "settings.open",
      label: T.settings,
      keywords: ["Settings", "Preferences"],
      run: deps.openSettings,
    },
    ...(installShellCommand
      ? [
          {
            id: "cli.install",
            label: T.cliInstall,
            keywords: ["Install Shell Command", "Create Command-line Launcher", "pitwall", "PATH"],
            // 실패 · 취소는 네이티브가 대화상자 · 로그로 알린다.
            run: () =>
              installShellCommand().catch((e: unknown) =>
                console.info("[cli] shell command not installed", e),
              ),
          } satisfies Action,
        ]
      : []),
    ...(["light", "dark", "system"] as const).map((preference): Action => ({
      id: `theme.${preference}`,
      label: T.theme(preference),
      keywords: [`Theme ${preference}`],
      run: () => deps.setTheme(preference),
    })),
    {
      id: "language.system",
      label: T.languageSystem,
      keywords: ["Language System", "언어"],
      run: setLanguage("system"),
    },
    ...deps.languages.map(({ locale, name }): Action => ({
      id: `language.${locale}`,
      label: T.language(name),
      keywords: [`Language ${locale}`, "언어"],
      run: setLanguage(locale),
    })),
  ];
}

/** 프로젝트 창의 툴 윈도우 열기 · 숨기기. 숨긴(닫은) 툴 윈도우를 다시 여는 길이다(project-layout.ts 머리 주석). */
export function toolWindowActions(getApi: () => DockviewApi | undefined): Action[] {
  const enabled = () => getApi() !== undefined;
  return [
    ...PANELS.filter((p) => p.home !== "editor").map((panel): Action => ({
      id: `toolWindow.${panel.id}`,
      label: `${panel.title} 툴 윈도우`,
      keywords: [`Tool Window ${panel.id}`],
      enabled,
      run: () => {
        const api = getApi();
        if (api) toggleToolWindow(api, panel.id);
      },
    })),
    {
      id: "toolWindow.hideAll",
      label: "모든 툴 윈도우 숨기기",
      keywords: ["Hide All Tool Windows"],
      enabled,
      run: () => {
        const api = getApi();
        if (api) collapseAllDocks(api);
      },
    },
  ];
}
