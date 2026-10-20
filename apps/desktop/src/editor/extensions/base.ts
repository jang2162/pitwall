// 뷰어의 기본 틀: 줄 번호, 현재 줄 표시, 선택 그리기.

import {
  drawSelection,
  highlightActiveLine,
  highlightActiveLineGutter,
  lineNumbers,
} from "@codemirror/view";

import type { EditorFeature } from ".";

export const base: EditorFeature = {
  id: "base",
  extension: () => [
    lineNumbers(),
    highlightActiveLineGutter(),
    highlightActiveLine(),
    drawSelection(),
  ],
};
