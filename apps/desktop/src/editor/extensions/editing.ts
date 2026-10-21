// 편집 키: Tab 들여쓰기와 네이티브 메뉴의 undo · redo. 글자 입력 · 지우기 · Enter 는 `navigation` 의 standardKeymap 이 한다.
// undo · redo 의 키(⌘Z · ⌘⇧Z, Win/Linux Ctrl+Z · Ctrl+Shift+Z)는 여기가 아니라 창의 키맵 액션 `editor.undo` · `editor.redo` 다
// (../editor-ops.ts) — keymap.toml 로 바꿀 수 있고, Win/Linux 의 Edit 메뉴가 같은 액션이다. 여기서도 잡으면 이벤트가 창까지 올라가
// 두 번 돈다(navigation.ts 머리 주석).
//
// undo 기록은 뷰가 아니라 문서에 있다(`context.history`, ../live-text.ts) — 같은 파일을 보이는 다른 탭에서 고친 것도, 탭을 옮겨
// 뷰를 다시 만들기 전에 고친 것도 되돌린다. macOS 네이티브 메뉴의 「실행 취소」(`undo:`)는 웹뷰에 `beforeinput`
// (`historyUndo`)으로 오므로 그것을 받는다. 읽기 전용 문서는 되돌리지 않는다 — 디스크에서 다시 읽은 것이 기록에 든다.

import { indentWithTab } from "@codemirror/commands";
import { EditorView, keymap } from "@codemirror/view";

import { type EditorFeatureContext, type EditorFeature } from ".";

/** 문서의 undo · redo. 읽기 전용이면 하지 않는다. 키맵 액션(../editor-ops.ts)도 이것을 쓴다. */
export function historyCommand(
  c: EditorFeatureContext,
  which: "undo" | "redo",
): (view: EditorView) => boolean {
  return (view) => !c.readOnly && c.history[which](view);
}

export const editing: EditorFeature = {
  id: "editing",
  extension: (c) => {
    const undo = historyCommand(c, "undo");
    const redo = historyCommand(c, "redo");
    return [
      keymap.of([indentWithTab]),
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
