// 테마 토큰(`grid.*`, src/theme/tokens.ts 의 GRID_TOKENS)을 그리드의 canvas 색으로 옮긴다(M3-16). 그리드는 CSS 변수를 읽지
// 못하므로(canvas) 테마 · 외관이 바뀌면 이 값이 바뀌고, 감싸개(glide-grid.tsx)가 그것을 Glide 의 `theme` 으로 넘겨 다시 칠한다.
//
// 돌려주는 객체의 키는 Glide 의 `Theme` 필드 이름이다 — Glide 를 import 하는 곳은 glide-grid.tsx 하나로 둔다(판을 바꿀 때 한 곳).

import type { TokenValues } from "../../theme/tokens";

/** 그리드 글꼴: 에디터와 같은 고정폭(10-ui.md 「UI 스택」 의 폰트 행). */
export const GRID_FONT_FAMILY = '"SF Mono", Menlo, Consolas, "DejaVu Sans Mono", monospace';

export interface GridColors {
  background: string;
  foreground: string;
  null: string;
  headerBackground: string;
  headerForeground: string;
  headerType: string;
  headerHover: string;
  border: string;
  selection: string;
  accent: string;
}

export function gridColors(values: TokenValues): GridColors {
  return {
    background: values["grid.background"],
    foreground: values["grid.foreground"],
    null: values["grid.null"],
    headerBackground: values["grid.headerBackground"],
    headerForeground: values["grid.headerForeground"],
    headerType: values["grid.headerType"],
    headerHover: values["grid.headerHover"],
    border: values["grid.border"],
    selection: values["grid.selection"],
    accent: values["grid.accent"],
  };
}

/** Glide `Theme` 의 부분(필드 이름이 같다). 정하지 않은 필드는 Glide 의 기본값이다. */
export function glideTheme(c: GridColors) {
  return {
    accentColor: c.accent,
    accentFg: c.background,
    accentLight: c.selection,
    textDark: c.foreground,
    textMedium: c.headerType,
    textLight: c.null,
    textBubble: c.foreground,
    bgIconHeader: c.headerType,
    fgIconHeader: c.headerBackground,
    textHeader: c.headerForeground,
    textHeaderSelected: c.background,
    bgCell: c.background,
    bgCellMedium: c.headerBackground,
    bgHeader: c.headerBackground,
    bgHeaderHasFocus: c.headerHover,
    bgHeaderHovered: c.headerHover,
    bgBubble: c.headerBackground,
    bgBubbleSelected: c.background,
    bgSearchResult: c.selection,
    borderColor: c.border,
    horizontalBorderColor: c.border,
    headerBottomBorderColor: c.border,
    drilldownBorder: c.border,
    linkColor: c.accent,
    resizeIndicatorColor: c.accent,
    cellHorizontalPadding: 8,
    cellVerticalPadding: 3,
    headerFontStyle: "600 12px",
    baseFontStyle: "12px",
    markerFontStyle: "11px",
    fontFamily: GRID_FONT_FAMILY,
    editorFontSize: "12px",
  };
}
