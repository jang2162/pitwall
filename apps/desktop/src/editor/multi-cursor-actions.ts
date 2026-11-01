// 멀티 커서의 키맵 액션(15-m4-plan M4-14, 10-ui §4.2 「멀티 커서」). 명령 · 상태는 extensions/multi-cursor.ts 에 있고 여기는
// 창의 키맵에 거는 자리다. 기본 키는 extensions/keymap-default/keymaps/default.toml 의 「멀티 커서」 묶음이고 모두 글자 영역
// (`editorFocus`)에서만 돈다 — 찾기 막대 안의 Esc 는 막대가 닫는다(../find-bar.tsx).
//
// `Esc`(커서 하나로)는 다른 처리기가 먼저 받은 Esc 에서는 돌지 않는다(`yieldsToHandled`): 찾기 칸 · CodeMirror 툴팁(자동완성
// 목록 등) · git 거터 팝업 · blame 메뉴가 Esc 로 닫으며 `preventDefault` 한 눌림이 창까지 올라와도 커서 · 선택을 접지 않는다.
// 그 처리기들은 Esc 를 받는 그 자리에서 자기를 닫으므로 「팝업이 열려 있는가」 로는 가를 수 없다. 창이 띄운 팝업은 키맵 엔진이
// 막는다(`popupOpen`). Esc 를 처리하는 새 처리기는 `preventDefault` 해야 이 규칙 안에 든다.

import type { StateCommand } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import type { Action } from "@pitwall/keymap";

import type { ProjectStrings } from "../layout/strings";
import type { NotificationInput } from "../notifications/notifications";
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

export function multiCursorActions(deps: {
  workspace: EditorWorkspace;
  strings: ProjectStrings["editor"]["actions"];
  /** 모든 일치 선택이 상한에 걸렸다고 알린다. 없으면 알리지 않는다(시험 · 키맵 표). */
  notify?: (input: NotificationInput) => void;
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
      run: command(
        state(
          selectAllOccurrences((cap) =>
            deps.notify?.({ level: "info", title: T.occurrencesCapped(cap) }),
          ),
        ),
      ),
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
        return v !== undefined && hasExtraCursors(v.state);
      },
      yieldsToHandled: true,
      run: command(singleCursor),
    },
  ];
}
