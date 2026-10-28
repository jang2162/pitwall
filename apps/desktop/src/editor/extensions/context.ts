// 뷰의 지금 context(`EditorFeatureContext`)를 담는 facet. 확장 목록(index.ts)과 따로 둔다 — 기능 모듈이 이것을 `"."` 에서 값으로
// 가져오면 index ↔ 기능이 순환해, 그 기능 모듈을 확장 목록보다 먼저 읽는 경로(diff 뷰의 revision-syntax.ts → highlight.ts 등)가
// 생기는 순간 `EDITOR_FEATURES` 에 `undefined` 가 든다.

import { Facet } from "@codemirror/state";

import type { EditorFeatureContext } from ".";

/** 뷰의 지금 context. 기능의 명령 · 리스너가 `state.facet(editorContext)` 로 읽는다. */
export const editorContext = Facet.define<EditorFeatureContext, EditorFeatureContext | undefined>({
  combine: (values) => values[0],
});
