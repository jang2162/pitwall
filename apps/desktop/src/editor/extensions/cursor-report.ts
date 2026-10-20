// 커서가 바뀌면 context 의 `onCursor` 로 알린다. 상태바의 커서 줄:열(M1-18)이 이것을 받는다 — 지금은 창의 에디터 작업 공간
// (`EditorWorkspace.onCursor`)까지만 간다.

import { EditorView } from "@codemirror/view";

import type { EditorFeature } from ".";

export const cursorReport: EditorFeature = {
  id: "cursorReport",
  extension: (c) =>
    EditorView.updateListener.of((update) => {
      if (!update.selectionSet) return;
      const head = update.state.selection.main.head;
      const line = update.state.doc.lineAt(head);
      c.onCursor({ line: line.number, column: head - line.from + 1 });
    }),
};
