// 지금 등록하는 액션(브리프 M0-13: 이미 있는 기능만). 기능이 없는 액션(Git · DB 등)은 그 기능과 함께 등록한다.
// 기본 키는 extensions/keymap-default/keymaps/default.toml.

import type { Action } from "@pitwall/keymap";
import type { DockviewApi } from "dockview-react";

import { collapseAllDocks, PANELS, toggleToolWindow } from "../layout/project-layout";
import type { ThemePreference } from "../theme/theme-store";

export function appActions(deps: {
  openFindAction(): void;
  openSettings(): Promise<void>;
  setTheme(preference: ThemePreference): void;
  /** 없으면(macOS 가 아니면) 액션을 등록하지 않는다. */
  installShellCommand?: () => Promise<void>;
}): Action[] {
  const { installShellCommand } = deps;
  return [
    {
      id: "action.find",
      label: "액션 찾기",
      keywords: ["Find Action", "command palette"],
      run: deps.openFindAction,
    },
    {
      id: "settings.open",
      label: "설정",
      keywords: ["Settings", "Preferences"],
      run: deps.openSettings,
    },
    ...(installShellCommand
      ? [
          {
            id: "cli.install",
            label: "Shell 명령 설치",
            keywords: ["Install Shell Command", "Create Command-line Launcher", "pitwall", "PATH"],
            // 실패 · 취소는 네이티브가 대화상자 · 로그로 알린다.
            run: () =>
              installShellCommand().catch((e: unknown) =>
                console.info("[cli] shell command not installed", e),
              ),
          } satisfies Action,
        ]
      : []),
    ...(
      [
        ["light", "라이트"],
        ["dark", "다크"],
        ["system", "시스템"],
      ] as const
    ).map(([preference, name]): Action => ({
      id: `theme.${preference}`,
      label: `테마: ${name}`,
      keywords: [`Theme ${preference}`],
      run: () => deps.setTheme(preference),
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
