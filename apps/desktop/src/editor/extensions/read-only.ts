// 읽기 전용: 이 앱이 고칠 수 없는 문서(크기 등급 · 인코딩 · 쓰기 권한, `context.readOnly`)는 사용자 입력을 막는다. 커서와 선택은
// 보여야 하므로 `editable` 은 끄지 않는다. 디스크에서 다시 읽은 내용을 넣는 것(live-text.ts)은 막지 않는다.

import { EditorState } from "@codemirror/state";

import type { EditorFeature } from ".";

export const readOnly: EditorFeature = {
  id: "readOnly",
  extension: (c) => EditorState.readOnly.of(c.readOnly),
};
