// 에디터 탭 액션. 모두 「활성 탭」(editor-workspace.ts)에 한다. 기본 키는 extensions/keymap-default/keymaps/default.toml
// (10-ui §4.2 「툴 윈도우 · 창」 · 「검색 · 이동」). 탭 우클릭 메뉴(editor-tab.tsx)도 이 액션을 부른다.
// 「Finder 에서 보기」(`editor.reveal`)와 「외부 도구로 열기」 는 외부 도구 실행을 가진 project-window.tsx 의 ProjectOpenWith 가 등록한다.
//
// `window.close`(macOS ⌘W, 네이티브 메뉴 「닫기」)는 에디터 탭이 있으면 활성 탭을 닫고, 없으면 창을 닫는다(VS Code 와 같다,
// 10-ui §5.2). 메뉴 항목이 OS 기본 「창 닫기」(performClose:)가 아니라 이 액션이라, 같은 ⌘W 가 키(웹뷰 keydown)와 메뉴
// accelerator 둘로 와도 둘 다 이 한 액션이고 키맵의 메뉴 중복 가드(`MENU_DUPLICATE_MS`)가 뒤의 것을 버린다. 키맵이 키를
// 보지 않는 경우(IME 조합 중)에 메뉴로만 와도 같은 규칙이라 탭이 있는 창이 통째로 닫히지 않는다. 팝업이 열린 동안에는 키도
// 메뉴도 이 액션을 돌리지 않는다(keymap-provider.tsx 의 창 팝업 규칙). 다른 창(웰컴 · 설정)의 `window.close` 는 창을 닫기만
// 한다(src/keymap/actions.ts).
//
// 탭을 닫는 액션은 모두 작업 공간의 `closeTabs` 를 지난다 — 그 문서를 보는 마지막 탭에 미저장 편집이 있으면 먼저 묻는다(M1-13).
// 탭이 없을 때의 `window.close` 는 창을 닫고, 창의 닫기 요청이 미저장을 묻는다(project-window.tsx).
// 「텍스트로 열기」 · 「hex 로 열기」 · 「기본 뷰어로 열기」(M1-23)는 활성 탭의 뷰어만 바꾼다(탭 params `viewer`) — 같은 파일의 다른
// 탭은 그대로다. `hex.gotoOffset`(⌘L, hex 뷰어에 포커스가 있을 때)은 활성 탭의 hex 뷰어에 오프셋 칸을 연다.
// `editor.save`(⌘S)는 활성 탭의 문서를 저장한다(자동 저장 없음, 결정 M1-D13). 네이티브 메뉴 「저장」 이 같은 액션이라 키와 메뉴
// accelerator 가 한 액션으로 모인다(`window.close` 와 같다) — 두 번 와도 둘째는 저장할 것이 없거나 첫 저장 뒤에 줄을 선다.

import type { Action } from "@pitwall/keymap";

import type { ProjectStrings } from "../layout/strings";
import {
  otherTabs,
  setTabViewer,
  splitRight,
  tabParams,
  toggleEditorMaximized,
  togglePinned,
} from "./editor-tabs";
import type { EditorWorkspace } from "./editor-workspace";
import { hexControls } from "./hex-controls";

export function editorActions(deps: {
  workspace: EditorWorkspace;
  strings: ProjectStrings["editor"];
  openSwitcher(): void;
  /** 이 창을 닫는다(`window.close` 가 탭이 없을 때). */
  closeWindow(): Promise<void>;
  /** 클립보드에 쓴다. 시험은 가짜를 넘긴다. */
  copyText(text: string): Promise<void>;
  onCopyFailed(error: unknown): void;
}): Action[] {
  const { workspace } = deps;
  const T = deps.strings.actions;
  const hasTab = () => workspace.activeTab() !== undefined;
  const withTab =
    (run: (panel: NonNullable<ReturnType<EditorWorkspace["activeTab"]>>) => void) => () => {
      const panel = workspace.activeTab();
      if (panel) run(panel);
    };
  return [
    {
      id: "window.close",
      label: T.closeTabOrWindow,
      keywords: ["Close", "Close Editor", "Close Window"],
      run: async () => {
        const panel = workspace.activeTab();
        if (panel) await workspace.closeTabs([panel]);
        else await deps.closeWindow();
      },
    },
    {
      id: "editor.closeTab",
      label: T.closeTab,
      keywords: ["Close Tab", "Close Editor"],
      enabled: hasTab,
      run: withTab((panel) => void workspace.closeTabs([panel])),
    },
    {
      id: "editor.closeOtherTabs",
      label: T.closeOtherTabs,
      keywords: ["Close Other Tabs", "Close Others"],
      enabled: hasTab,
      run: withTab((panel) => void workspace.closeTabs(otherTabs(panel))),
    },
    {
      id: "editor.save",
      label: T.save,
      keywords: ["Save", "Save File"],
      enabled: hasTab,
      run: withTab((panel) => void workspace.save(panel)),
    },
    {
      id: "editor.saveAll",
      label: T.saveAll,
      keywords: ["Save All"],
      run: () => void workspace.saveAll(),
    },
    {
      id: "editor.splitRight",
      label: T.splitRight,
      keywords: ["Split Right", "Split Vertically"],
      enabled: hasTab,
      run: withTab((panel) => {
        const api = workspace.api();
        if (api) workspace.focus(splitRight(api, panel).id);
      }),
    },
    {
      id: "editor.togglePin",
      label: T.togglePin,
      keywords: ["Pin Tab", "Unpin Tab"],
      enabled: hasTab,
      run: withTab(togglePinned),
    },
    {
      id: "editor.toggleMaximized",
      label: T.toggleMaximized,
      keywords: ["Maximize Editor", "Hide All Tool Windows"],
      enabled: () => workspace.api() !== undefined,
      run: () => {
        const api = workspace.api();
        if (api) toggleEditorMaximized(api);
      },
    },
    {
      id: "editor.copyPath",
      label: T.copyPath,
      keywords: ["Copy Path", "Copy Absolute Path"],
      enabled: hasTab,
      run: withTab((panel) => {
        deps.copyText(tabParams(panel).path).catch(deps.onCopyFailed);
      }),
    },
    {
      id: "editor.openAsText",
      label: T.openAsText,
      keywords: ["Open as Text", "Open as Plain Text"],
      enabled: hasTab,
      run: withTab((panel) => setTabViewer(panel.api, tabParams(panel), "text")),
    },
    {
      id: "editor.openAsHex",
      label: T.openAsHex,
      keywords: ["Open as Hex", "Hex Viewer", "Binary"],
      enabled: hasTab,
      run: withTab((panel) => setTabViewer(panel.api, tabParams(panel), "hex")),
    },
    {
      id: "editor.openAsDefault",
      label: T.openAsDefault,
      keywords: ["Open with Default Viewer"],
      enabled: () => {
        const panel = workspace.activeTab();
        return panel !== undefined && tabParams(panel).viewer !== undefined;
      },
      run: withTab((panel) => setTabViewer(panel.api, tabParams(panel), undefined)),
    },
    {
      id: "hex.gotoOffset",
      label: T.gotoOffset,
      keywords: ["Go to Offset", "Hex"],
      enabled: () => {
        const panel = workspace.activeTab();
        return panel !== undefined && hexControls.has(panel.id);
      },
      run: withTab((panel) => hexControls.get(panel.id)?.gotoOffset()),
    },
    {
      id: "editor.switcher",
      label: T.switcher,
      keywords: ["Switcher", "Recent Tabs"],
      enabled: hasTab,
      run: deps.openSwitcher,
    },
  ];
}
