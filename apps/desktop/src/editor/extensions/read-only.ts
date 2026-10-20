// 읽기 전용. 지금 뷰어는 모든 문서를 읽기 전용으로 연다 — 편집은 M1-13 이 `context.readOnly`(크기 등급 · 인코딩 · 쓰기 권한)로
// 바꾼다. 커서와 선택은 보여야 하므로 `editable` 은 끄지 않는다.

import { EditorState } from "@codemirror/state";

import type { EditorFeature } from ".";

export const readOnly: EditorFeature = {
  id: "readOnly",
  extension: () => EditorState.readOnly.of(true),
};
