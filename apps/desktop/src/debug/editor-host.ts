// 창의 디버그 에디터 호스트를 거는 자리(debug-feature.tsx 가 건다). 텍스트 뷰어는 이것만 import 한다 — 호스트를 만드는 쪽은 툴
// 윈도우까지 끌고 와서, 텍스트 뷰어가 그것을 import 하면 확장 목록(editor/extensions/index.ts)과 순환한다(git 거터의 host-store.ts 와 같다).

import { create } from "zustand";

import type { EditorDebug } from "./breakpoint-gutter";

/** 이 창의 디버그 에디터 호스트. 없으면(웰컴 · 파일 창 · 시험) 브레이크포인트 거터가 꺼진다. */
export const debugEditorHost = create<{ host: EditorDebug | undefined }>(() => ({
  host: undefined,
}));
