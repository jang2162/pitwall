// 터미널 액션(⌘⇧A 로도 찾는다). 기본 키는 extensions/keymap-default/keymaps/default.toml 의 「터미널」 묶음 — 모두 `when = "terminalFocus"`
// 라 터미널이 포커스를 가졌을 때만 키로 돈다(terminal-keys.ts).

import type { Action } from "@pitwall/keymap";

import type { ProjectStrings } from "../layout/strings";
import type { TerminalTabs } from "./terminal-tabs";

export interface TerminalActionDeps {
  strings: ProjectStrings["terminal"];
  tabs: TerminalTabs;
  /** Host 에 연결돼 프로젝트가 있어 새 셸을 띄울 수 있는가. */
  canOpen(): boolean;
  /** 다음 프레임에 활성 터미널에 포커스를 준다(새 탭은 다음 렌더에서 화면에 붙는다). */
  focusActive(): void;
  startRename(id: number): void;
  writeClipboard(text: string): Promise<void>;
  readClipboard(): Promise<string>;
  onClipboardFailed(title: string, error: unknown): void;
}

export function terminalActions(deps: TerminalActionDeps): Action[] {
  const { strings: T, tabs } = deps;
  const activeId = () => tabs.store.getState().activeId;
  return [
    {
      id: "terminal.newTab",
      label: T.actions.newTab,
      keywords: ["New Terminal Tab", "New Session"],
      enabled: deps.canOpen,
      run: () => {
        tabs.open();
        deps.focusActive();
      },
    },
    {
      id: "terminal.closeTab",
      label: T.actions.closeTab,
      keywords: ["Close Terminal Tab", "Close Session"],
      enabled: () => activeId() !== undefined,
      run: () => {
        const id = activeId();
        if (id === undefined) return;
        tabs.close(id);
        deps.focusActive();
      },
    },
    {
      id: "terminal.renameTab",
      label: T.actions.renameTab,
      keywords: ["Rename Terminal Tab", "Rename Session"],
      enabled: () => activeId() !== undefined,
      run: () => {
        const id = activeId();
        if (id !== undefined) deps.startRename(id);
      },
    },
    {
      id: "terminal.copy",
      label: T.actions.copy,
      keywords: ["Copy"],
      enabled: () => tabs.active()?.term.hasSelection() ?? false,
      run: () => {
        const text = tabs.active()?.term.getSelection();
        if (!text) return;
        deps.writeClipboard(text).catch((e: unknown) => deps.onClipboardFailed(T.copyFailed, e));
      },
    },
    {
      id: "terminal.paste",
      label: T.actions.paste,
      keywords: ["Paste"],
      enabled: () => tabs.active() !== undefined,
      run: () => {
        deps
          .readClipboard()
          .then((text) => tabs.active()?.paste(text))
          .catch((e: unknown) => deps.onClipboardFailed(T.pasteFailed, e));
      },
    },
  ];
}
