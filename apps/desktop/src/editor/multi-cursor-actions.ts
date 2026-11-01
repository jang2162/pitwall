// 멀티 커서의 키맵 액션(15-m4-plan M4-14, 10-ui §4.2 「멀티 커서」). 명령 · 상태는 extensions/multi-cursor.ts 에 있고 여기는
// 창의 키맵에 거는 자리다. 기본 키는 extensions/keymap-default/keymaps/default.toml 의 「멀티 커서」 묶음이고 모두 글자 영역
// (`editorFocus`)에서만 돈다 — 찾기 막대 안의 Esc 는 막대가 닫는다(../find-bar.tsx).
//
// `Esc`(커서 하나로)는 에디터에 팝업이 떠 있으면 꺼져 키를 그 팝업에 둔다: 창이 띄운 팝업은 키맵 엔진이 막고(`popupOpen`),
// 에디터 안의 CodeMirror 툴팁(자동완성 목록 등)은 여기서 `.cm-tooltip` 으로 본다 — 툴팁의 Esc 는 CodeMirror 키맵이 받고
// 이벤트는 창까지 올라와 이 액션이 또 돌기 때문이다.

import type { StateCommand } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import type { Action } from "@pitwall/keymap";

import type { ProjectStrings } from "../layout/strings";
import { targetView } from "./editor-ops";
import type { EditorWorkspace } from "./editor-workspace";
import {
  addCursorAbove,
  addCursorBelow,
  hasExtraCursors,
  selectAllOccurrences,
  selectNextOccurrence,
  singleCursor,
  toggleColumnMode,
  unselectOccurrence,
} from "./extensions/multi-cursor";

function hasPopup(view: EditorView): boolean {
  return view.dom.querySelector(".cm-tooltip") !== null;
}

export function multiCursorActions(deps: {
  workspace: EditorWorkspace;
  strings: ProjectStrings["editor"]["actions"];
}): Action[] {
  const T = deps.strings;
  const view = () => targetView(deps.workspace);
  const hasView = () => view() !== undefined;
  const command = (run: (view: EditorView) => boolean) => () => {
    const v = view();
    if (v) run(v);
  };
  const state = (run: StateCommand) => (v: EditorView) =>
    run({ state: v.state, dispatch: (tr) => v.dispatch(tr) });
  return [
    {
      id: "editor.selectNextOccurrence",
      label: T.selectNextOccurrence,
      keywords: ["Add Selection for Next Occurrence", "Multiple Carets"],
      enabled: hasView,
      repeat: true,
      run: command(state(selectNextOccurrence)),
    },
    {
      id: "editor.unselectOccurrence",
      label: T.unselectOccurrence,
      keywords: ["Unselect Occurrence", "Multiple Carets"],
      enabled: hasView,
      repeat: true,
      run: command(state(unselectOccurrence)),
    },
    {
      id: "editor.selectAllOccurrences",
      label: T.selectAllOccurrences,
      keywords: ["Select All Occurrences", "Multiple Carets"],
      enabled: hasView,
      run: command(state(selectAllOccurrences)),
    },
    {
      id: "editor.addCursorAbove",
      label: T.addCursorAbove,
      keywords: ["Clone Caret Above", "Multiple Carets"],
      enabled: hasView,
      repeat: true,
      run: command(state(addCursorAbove)),
    },
    {
      id: "editor.addCursorBelow",
      label: T.addCursorBelow,
      keywords: ["Clone Caret Below", "Multiple Carets"],
      enabled: hasView,
      repeat: true,
      run: command(state(addCursorBelow)),
    },
    {
      id: "editor.toggleColumnMode",
      label: T.toggleColumnMode,
      keywords: ["Column Selection Mode", "Block Selection"],
      enabled: hasView,
      run: command(state(toggleColumnMode)),
    },
    {
      id: "editor.singleCursor",
      label: T.singleCursor,
      keywords: ["Remove Secondary Carets", "Escape"],
      enabled: () => {
        const v = view();
        return v !== undefined && !hasPopup(v) && hasExtraCursors(v.state);
      },
      run: command(singleCursor),
    },
  ];
}
