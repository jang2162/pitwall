// 지금 등록하는 액션(브리프 M0-13: 이미 있는 기능만). 기능이 없는 액션(Git · DB 등)은 그 기능과 함께 등록한다.
// 기본 키는 extensions/keymap-default/keymaps/default.toml.

import type { Action } from "@pitwall/keymap";
import type { DockviewApi } from "dockview-react";

import type { AppStrings } from "../i18n/app-strings";
import {
  type Arrow,
  collapseAllDocks,
  lastToolWindow,
  PANELS,
  resizeToolWindow,
  showPanel,
  toggleToolWindow,
} from "../layout/project-layout";
import type { ProjectStrings } from "../layout/strings";
import type { ThemePreference } from "../theme/theme-store";

export function appActions(deps: {
  strings: AppStrings;
  /** 등록된 UI 언어(그 언어로 쓴 이름). 언어마다 액션 하나와 「시스템」 액션을 둔다. */
  languages: readonly { locale: string; name: string }[];
  setLanguage(language: string): Promise<void>;
  openFindAction(): void;
  /** Search Everywhere 를 전체 탭으로 연다(⇧⇧). */
  openSearchEverywhere(): void;
  openSettings(): Promise<void>;
  setTheme(preference: ThemePreference): void;
  /** 없으면(macOS 가 아니면) 액션을 등록하지 않는다. */
  installShellCommand?: () => Promise<void>;
  /**
   * 있으면 `window.close`(⌘W · 메뉴 「닫기」)로 이 창을 닫는다. 프로젝트 창은 넘기지 않는다 — 그 창의 `window.close` 는 탭을
   * 먼저 닫는다(src/editor/editor-actions.ts).
   */
  closeWindow?: () => Promise<void>;
}): Action[] {
  const { installShellCommand, closeWindow } = deps;
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
      id: "search.everywhere",
      label: T.searchEverywhere,
      keywords: ["Search Everywhere"],
      run: deps.openSearchEverywhere,
    },
    {
      id: "settings.open",
      label: T.settings,
      keywords: ["Settings", "Preferences"],
      run: deps.openSettings,
    },
    ...(closeWindow
      ? [
          {
            id: "window.close",
            label: T.closeWindow,
            keywords: ["Close Window"],
            run: closeWindow,
          } satisfies Action,
        ]
      : []),
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

/** 포커스가 든 툴 윈도우의 id(`data-tool-window`, src/keymap/context.ts 의 툴 윈도우 층). */
function focusedToolWindow(): string | undefined {
  return (
    document.activeElement?.closest("[data-tool-window]")?.getAttribute("data-tool-window") ??
    undefined
  );
}

/** 툴 윈도우의 내용에 포커스를 둔다. 막 다시 연 툴 윈도우는 다음 프레임에 그려진다. */
function focusToolWindow(id: string) {
  requestAnimationFrame(() =>
    document.querySelector<HTMLElement>(`[data-tool-window="${CSS.escape(id)}"]`)?.focus(),
  );
}

const ARROWS: readonly { arrow: Arrow; name: "Left" | "Right" | "Up" | "Down" }[] = [
  { arrow: "left", name: "Left" },
  { arrow: "right", name: "Right" },
  { arrow: "up", name: "Up" },
  { arrow: "down", name: "Down" },
];

/**
 * 프로젝트 창의 툴 윈도우 열기 · 숨기기 · 레이아웃 액션(10-ui 「툴 윈도우 · 창」). 숨긴(닫은) 툴 윈도우를 다시 여는 길이다
 * (project-layout.ts 머리 주석). `focusEditor` 는 「에디터로 포커스」(Esc) — 활성 에디터 탭에 포커스를 두고, 탭이 없으면 `false`.
 */
export function toolWindowActions(
  getApi: () => DockviewApi | undefined,
  strings: ProjectStrings,
  { focusEditor }: { focusEditor?: { enabled(): boolean; run(): void } } = {},
): Action[] {
  const enabled = () => getApi() !== undefined;
  const T = strings.layoutActions;
  return [
    ...PANELS.map((panel): Action => ({
      id: `toolWindow.${panel.id}`,
      label: strings.toolWindowAction(strings.toolWindows[panel.id] ?? panel.id),
      keywords: [`Tool Window ${panel.id}`],
      enabled,
      run: () => {
        const api = getApi();
        if (api) toggleToolWindow(api, panel.id);
      },
    })),
    {
      id: "toolWindow.hideAll",
      label: strings.hideAllToolWindows,
      keywords: ["Hide All Tool Windows"],
      enabled,
      run: () => {
        const api = getApi();
        if (api) collapseAllDocks(api);
      },
    },
    {
      id: "toolWindow.last",
      label: T.lastToolWindow,
      keywords: ["Jump to Last Tool Window"],
      enabled,
      run: () => {
        const api = getApi();
        if (!api) return;
        const id = lastToolWindow(api);
        showPanel(api, id);
        focusToolWindow(id);
      },
    },
    ...ARROWS.map(({ arrow, name }): Action => ({
      id: `toolWindow.stretch${name}`,
      label: T.stretch[arrow],
      keywords: [`Stretch to ${name}`, `Resize Tool Window ${name}`],
      // 포커스가 툴 윈도우에 있을 때만(키맵의 `when` 도 그렇다).
      enabled: () => enabled() && focusedToolWindow() !== undefined,
      run: () => {
        const api = getApi();
        const id = focusedToolWindow();
        if (api && id) resizeToolWindow(api, id, arrow);
      },
    })),
    ...(focusEditor
      ? [
          {
            id: "editor.focus",
            label: T.focusEditor,
            keywords: ["Focus Editor", "Jump to Editor"],
            enabled: focusEditor.enabled,
            run: focusEditor.run,
          } satisfies Action,
        ]
      : []),
  ];
}
