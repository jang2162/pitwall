// 테마 토큰(`grid.*`, src/theme/tokens.ts 의 GRID_TOKENS)을 그리드의 canvas 색으로 옮긴다(M3-16). 그리드는 CSS 변수를 읽지
// 못하므로(canvas) 테마 · 외관이 바뀌면 이 값이 바뀌고, 감싸개(glide-grid.tsx)가 그것을 Glide 의 `theme` 으로 넘겨 다시 칠한다.
//
// 돌려주는 객체의 키는 Glide 의 `Theme` 필드 이름이다 — Glide 를 import 하는 곳은 glide-grid.tsx 하나로 둔다(판을 바꿀 때 한 곳).

import type { TokenValues } from "../../theme/tokens";

/** 그리드 글꼴: 에디터와 같은 고정폭(10-ui.md 「UI 스택」 의 폰트 행). */
export const GRID_FONT_FAMILY = '"SF Mono", Menlo, Consolas, "DejaVu Sans Mono", monospace';

/** 셀 글자 크기와 NULL 셀의 글꼴(기울임 — 색만으로 글자 「NULL」 과 가르지 않는다, 10-ui.md §6). */
const BASE_FONT_STYLE = "12px";
export const NULL_FONT_STYLE = `italic ${BASE_FONT_STYLE}`;

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
  /**
   * 편집 대기 셀의 배경(테이블 데이터 M3-17): 바뀐 셀 · 추가 행 · 지울 행. diff 토큰(`diff.line.*` — 반투명)을 그리드 배경 위에
   * 미리 섞은 불투명 색이다(대비를 그 색으로 잰다 — tokens.test.ts).
   */
  marks: Record<GridCellMarkKind, string>;
}

/** 편집 대기 표지의 종류(glide-grid.tsx 의 `cellMark`). */
export type GridCellMarkKind = "changed" | "inserted" | "deleted";

/** `#rrggbb` · `#rrggbbaa` 색을 불투명한 `base`(`#rrggbb`) 위에 섞은 불투명 `#rrggbb`. 다른 꼴이면 `color` 그대로. */
export function opaqueOver(color: string, base: string): string {
  const c = /^#([0-9a-f]{6})([0-9a-f]{2})?$/i.exec(color);
  const b = /^#([0-9a-f]{6})$/i.exec(base);
  if (!c || !b) return color;
  const alpha = c[2] === undefined ? 1 : parseInt(c[2], 16) / 255;
  const channel = (hex: string, i: number) => parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  const out = [0, 1, 2].map((i) =>
    Math.round(channel(c[1]!, i) * alpha + channel(b[1]!, i) * (1 - alpha))
      .toString(16)
      .padStart(2, "0"),
  );
  return `#${out.join("")}`;
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
    marks: {
      changed: opaqueOver(values["diff.line.changed"], values["grid.background"]),
      inserted: opaqueOver(values["diff.line.added"], values["grid.background"]),
      deleted: opaqueOver(values["diff.line.deleted"], values["grid.background"]),
    },
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
    baseFontStyle: BASE_FONT_STYLE,
    markerFontStyle: "11px",
    fontFamily: GRID_FONT_FAMILY,
    editorFontSize: "12px",
  };
}
