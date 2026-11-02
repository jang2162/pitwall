// LSP 기능 `edit`(15-m4-plan M4-12 — `LSP_FEATURES` 의 한 줄): 문서를 창의 편집 문맥에 올리고(동기화한 문서 표 — 적용기의 미리
// 확인이 그 버전을 본다), rename 입력 · 코드 액션 전구 · 진단 호버의 「빠른 수정」(M4-07 팝업 바탕의 `actions` 칸)을 단다. 키
// (`⇧F6` · `⌥↩`)는 창의 액션이다(edit-ui.tsx).

import { ViewPlugin } from "@codemirror/view";

import { codeActionTheme, lightbulb, showCodeActions } from "./code-actions";
import { editWindowOf } from "./edit-window";
import type { LspEditorFeature } from "./features";
import { popupActions } from "./popup";
import { renameInputField, renameTheme } from "./rename";

export const lspEdit: LspEditorFeature = {
  id: "edit",
  extension: (_context, doc) => [
    // 탭이 보이는 동안 이 창이 서버에 동기화한 문서다.
    ViewPlugin.define(() => {
      const off = editWindowOf(doc.services)?.register(doc);
      return { destroy: () => off?.() };
    }),
    renameInputField,
    renameTheme,
    codeActionTheme,
    lightbulb(doc),
    popupActions.of({
      kinds: ["diagnostic"],
      actions: (popup) => {
        const win = editWindowOf(doc.services);
        if (!win || !doc.capabilities.code_action) return [];
        return [
          {
            id: "quickFix",
            label: win.strings().quickFix,
            run: (view) =>
              void showCodeActions(view, win, {
                from: popup.pos,
                to: popup.end ?? popup.pos,
                diagnostic: popup.data,
                only: ["quickfix"],
              }),
          },
        ];
      },
    }),
  ],
};
