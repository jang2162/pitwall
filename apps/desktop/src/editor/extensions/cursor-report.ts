// 커서가 바뀌면 context 의 `onCursor` 로 알린다. 상태바의 커서 줄:열(M1-18)과 내비게이션 기록의 「큰 이동」(M4-09,
// ../navigation.ts)이 이것을 받는다 — 창의 에디터 작업 공간(`EditorWorkspace.reportCursor`)으로 간다.

import { EditorView } from "@codemirror/view";

import type { EditorFeature } from ".";

export const cursorReport: EditorFeature = {
  id: "cursorReport",
  extension: (c) =>
    EditorView.updateListener.of((update) => {
      if (!update.selectionSet) return;
      const head = update.state.selection.main.head;
      const line = update.state.doc.lineAt(head);
      // 편집이 민 커서와 커서가 여럿인 선택(바뀌기 전이나 뒤 — 커서 더하기 · 빼기)은 내비게이션 기록의 「큰 이동」 이 아니다
      // (../navigation.ts). 기록이 보는 자리는 주 커서다.
      const quiet =
        update.docChanged ||
        update.startState.selection.ranges.length > 1 ||
        update.state.selection.ranges.length > 1;
      c.onCursor({ line: line.number, column: head - line.from + 1 }, quiet);
    }),
};
