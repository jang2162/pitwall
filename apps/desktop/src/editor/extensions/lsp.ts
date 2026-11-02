// LSP 기능 칸(15-m4-plan M4-07 ③): 문서가 언어 서버에 열렸으면(`context.lsp` — 탭이 잡은 LSP 세션, src/lsp/lsp-sessions.ts)
// 팝업 바탕(src/lsp/popup.ts) · 한글 조합 쥐기와 LSP 기능 표(`LSP_FEATURES`, src/lsp/features.ts)를 펼친다. 열리지 않은 문서(git ·
// DB 문서 · 편집 등급 밖 · 파일 창)는 빈 칸이다. 기능 행은 이 파일이 아니라 `LSP_FEATURES` 에 한 줄씩 끼운다.
//
// 한글 IME 조합(⑥): 이 뷰가 조합을 시작하면(`compositionstart` — CodeMirror 의 `compositionStarted`) 문서의 바뀜을 쥐고, 조합이
// 끝나면(`compositionend` 뒤 한 매크로태스크 — CodeMirror 는 끝 이벤트 뒤 마이크로태스크에서 마지막 조합 트랜잭션을 낸다) 놓는다 —
// 조합 중의 글자는 언어 서버에 가지 않고 끝난 뒤 한 번 간다. 다른 뷰(같은 문서의 다른 탭)가 받은 조합 트랜잭션은 보지 않는다 — 그
// 뷰는 조합하지 않으므로 끝 이벤트도 받지 않는다. 끝 이벤트를 잃었으면(조합이 아닌 다음 업데이트) 그때 놓는다.

import { ViewPlugin, type ViewUpdate } from "@codemirror/view";

import { LSP_FEATURES } from "../../lsp/features";
import type { LspDocument } from "../../lsp/lsp-sessions";
import { popupBase } from "../../lsp/popup";
import type { EditorFeature } from ".";

function compositionHold(doc: LspDocument) {
  return ViewPlugin.define((view) => {
    let release: (() => void) | undefined;
    let ending: ReturnType<typeof setTimeout> | undefined;
    const end = () => {
      clearTimeout(ending);
      ending = undefined;
      release?.();
      release = undefined;
    };
    const start = () => {
      clearTimeout(ending);
      ending = undefined;
      release ??= doc.hold();
    };
    const onStart = () => start();
    const onEnd = () => {
      clearTimeout(ending);
      ending = setTimeout(() => {
        ending = undefined;
        if (!view.compositionStarted) end();
      }, 0);
    };
    view.contentDOM.addEventListener("compositionstart", onStart);
    view.contentDOM.addEventListener("compositionend", onEnd);
    return {
      update(u: ViewUpdate) {
        if (u.view.compositionStarted) start();
        else if (release && ending === undefined && u.docChanged) end();
      },
      destroy() {
        view.contentDOM.removeEventListener("compositionstart", onStart);
        view.contentDOM.removeEventListener("compositionend", onEnd);
        end();
      },
    };
  });
}

export const lsp: EditorFeature = {
  id: "lsp",
  extension: (c) => {
    const doc = c.lsp;
    if (!doc) return [];
    return [popupBase, compositionHold(doc), ...LSP_FEATURES.map((f) => f.extension(c, doc))];
  },
};
