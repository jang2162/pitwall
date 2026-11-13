// 뷰어의 기본 틀: 줄 번호, 현재 줄 표시, 선택 그리기.

import {
  drawSelection,
  highlightActiveLine,
  highlightActiveLineGutter,
  lineNumbers,
} from "@codemirror/view";

import type { EditorFeature } from ".";
import { editorTooltips } from "../tooltip-layer";

export const base: EditorFeature = {
  id: "base",
  extension: () => [
    lineNumbers(),
    highlightActiveLineGutter(),
    highlightActiveLine(),
    drawSelection(),
    // 툴팁은 패널에 잘리지 않게 body 의 툴팁 층에(../tooltip-layer.ts).
    editorTooltips(),
  ],
};
