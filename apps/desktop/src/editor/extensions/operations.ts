// 에디터 조작(12-m1-plan M1-18): 키맵 컨텍스트, 찾기 · 바꾸기 패널의 상태, 코드 글꼴.
//
// - 키맵 컨텍스트: 글자 영역(contentDOM)이 `editorFocus` 를, 찾기 막대가 `editorFind` 를 낸다(src/keymap/context.ts). 편집 키
//   (줄 복제 · 줄 삭제 · 줄 이동 · 줄 주석 · 줄로 이동)는 `editorFocus` 에서만 돌아 찾기 칸의 ⌘⌫ 는 글자를 지운다. 키와 실행은 창의
//   키맵 액션이다(../editor-ops.ts).
// - 찾기: `@codemirror/search` 의 쿼리 · 일치 강조 · 다음 찾기 · 바꾸기를 쓰고 패널만 우리 것이다 — 뷰어가 `createFindPanel` 의
//   자리에 React 찾기 막대를 그린다(../find-bar.tsx). 찾기 / 바꾸기 중 어느 쪽으로 열었는지는 `findMode` 가 갖는다.
// - 글꼴: 설정 값이 있을 때만 기본(`theme` 의 시스템 고정폭 · 13px)을 덮는다.
// - 상태바의 커서 · 선택(../editor-status.tsx): `cursorReport` 는 선택이 바뀔 때만 알린다. 뷰가 처음 생길 때와, 다른 탭에서 고쳐
//   이 뷰의 커서 줄이 밀릴 때(선택은 그대로)도 `onCursor` 로 알린다.

import { search } from "@codemirror/search";
import { type Extension, Prec, StateEffect, StateField } from "@codemirror/state";
import { EditorView, ViewPlugin } from "@codemirror/view";

import type { EditorFeature, EditorFont } from ".";

/** 글자 영역의 키맵 컨텍스트 키. */
export const EDITOR_FOCUS = "editorFocus";
/** 찾기 막대의 키맵 컨텍스트 키. */
export const EDITOR_FIND = "editorFind";

export type FindMode = "find" | "replace";

export const setFindMode = StateEffect.define<FindMode>();

/** 찾기 막대가 바꾸기 줄을 보이는가. */
export const findMode = StateField.define<FindMode>({
  create: () => "find",
  update: (mode, tr) => {
    for (const e of tr.effects) if (e.is(setFindMode)) mode = e.value;
    return mode;
  },
});

// 같은 글꼴이면 같은 테마를 쓴다 — 다른 값(언어 · 외관)으로 다시 구성될 때마다 스타일 규칙이 새로 쌓이지 않게. 값의 가짓수는
// 사람이 설정에 적은 만큼이다.
const fontThemes = new Map<string, Extension>();

function fontTheme({ family, size }: EditorFont): Extension {
  if (family === undefined && size === undefined) return [];
  const key = `${family ?? ""}\n${size ?? ""}`;
  let theme = fontThemes.get(key);
  if (!theme) {
    theme = Prec.high(
      EditorView.theme({
        ...(size !== undefined && { "&": { fontSize: `${size}px` } }),
        ...(family !== undefined && { ".cm-scroller": { fontFamily: family } }),
      }),
    );
    fontThemes.set(key, theme);
  }
  return theme;
}

function cursorOf(view: EditorView) {
  const head = view.state.selection.main.head;
  const line = view.state.doc.lineAt(head);
  return { line: line.number, column: head - line.from + 1 };
}

export const operations: EditorFeature = {
  id: "operations",
  extension: (c) => [
    EditorView.contentAttributes.of({ "data-keymap-context": EDITOR_FOCUS }),
    ViewPlugin.define((view) => {
      c.onCursor(cursorOf(view));
      return {
        update: (u) => {
          if (u.docChanged && !u.selectionSet) c.onCursor(cursorOf(u.view));
        },
      };
    }),
    findMode,
    search({ top: true, createPanel: c.createFindPanel }),
    fontTheme(c.font),
  ],
};
