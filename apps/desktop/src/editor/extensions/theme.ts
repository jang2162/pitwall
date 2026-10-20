// 에디터 색 · 글꼴. 색은 테마 토큰의 CSS 변수(src/theme/tokens.ts)로만 읽어 테마가 바뀌면 다시 만들지 않아도 따라 바뀐다. 글꼴은
// 시스템 고정폭(결정 M1-D17).

import { EditorView } from "@codemirror/view";

import type { EditorFeature } from ".";

export const CODE_FONT = '"SF Mono", Menlo, Consolas, "DejaVu Sans Mono", monospace';

const editorTheme = EditorView.theme({
  "&": {
    height: "100%",
    backgroundColor: "var(--pw-editor-background)",
    color: "var(--pw-editor-foreground)",
    fontSize: "13px",
  },
  ".cm-scroller": { fontFamily: CODE_FONT, lineHeight: "1.5" },
  ".cm-content": { caretColor: "var(--pw-editor-foreground)" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--pw-editor-foreground)" },
  ".cm-gutters": {
    backgroundColor: "var(--pw-editor-background)",
    color: "var(--pw-ui-mutedForeground)",
    border: "none",
  },
  ".cm-activeLine, .cm-activeLineGutter": { backgroundColor: "var(--pw-ui-hoverBackground)" },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground": {
    backgroundColor: "var(--pw-ui-activeBackground)",
  },
});

export const theme: EditorFeature = {
  id: "theme",
  extension: () => editorTheme,
};
