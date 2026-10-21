// 편집 키: undo · redo 와 Tab 들여쓰기. 글자 입력 · 지우기 · Enter 는 `navigation` 의 standardKeymap 이 한다.
//
// undo 기록은 뷰가 아니라 문서에 있다(`context.history`, ../live-text.ts) — 같은 파일을 보이는 다른 탭에서 고친 것도, 탭을 옮겨
// 뷰를 다시 만들기 전에 고친 것도 되돌린다. 네이티브 메뉴의 「실행 취소」(macOS `undo:`)는 웹뷰에 `beforeinput`
// (`historyUndo`)으로 오므로 그것도 받는다. 읽기 전용 문서는 되돌리지 않는다 — 디스크에서 다시 읽은 것이 기록에 든다.

import { indentWithTab } from "@codemirror/commands";
import { EditorView, keymap } from "@codemirror/view";

import type { EditorFeature } from ".";

export const editing: EditorFeature = {
  id: "editing",
  extension: (c) => {
    const undo = (view: EditorView) => !c.readOnly && c.history.undo(view);
    const redo = (view: EditorView) => !c.readOnly && c.history.redo(view);
    return [
      keymap.of([
        { key: "Mod-z", run: undo, preventDefault: true },
        { key: "Mod-y", mac: "Mod-Shift-z", run: redo, preventDefault: true },
        { linux: "Ctrl-Shift-z", run: redo, preventDefault: true },
        indentWithTab,
      ]),
      EditorView.domEventHandlers({
        beforeinput(event, view) {
          const run =
            event.inputType === "historyUndo"
              ? undo
              : event.inputType === "historyRedo"
                ? redo
                : undefined;
          if (!run) return false;
          event.preventDefault();
          return run(view);
        },
      }),
    ];
  },
};
