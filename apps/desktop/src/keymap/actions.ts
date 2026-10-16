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
}): Action[] {
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
