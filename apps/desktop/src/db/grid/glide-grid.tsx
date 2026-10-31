// Glide Data Grid 감싸개(M3-16, 10-ui.md 「UI 스택」 · 결정 M3-D7). **Glide 를 import 하는 곳은 이 파일 하나다** — 판을 바꾸거나
// 갈아 끼울 때 여기만 고친다(메인 판정, 2026-10-31). 판은 6.0.4-alpha24 로 고정했다(안정판 6.0.3 은 React 19 peer 밖이고 전이
// 의존성에 CC-BY-4.0 이 든다 — 10-ui.md 「UI 스택」 의 DB 그리드 행 비고).
//
// - 행은 `ResultBuffer`(React 상태 밖)에서 화면 범위만 읽는다. 버퍼가 바뀌면 행 수만 상태로 올리고, 정렬 · 필터로 뷰가 바뀌면
//   셀 함수를 새로 만들어 다시 칠한다.
// - **셀은 늘 글자(`Text`) 칸이다.** DB 값은 신뢰할 수 없는 글이라 HTML 로 그리는 칸(markdown · uri · image)을 쓰지 않고,
//   오버레이 편집기(그 지연 청크에 marked · 이미지 캐러셀이 있다)도 열지 않는다(`allowOverlay: false`).
// - **결과에 묶인 상태는 결과 id 로 둔다**: 열 순서 · 너비 · 고르기와 Glide 안의 상태(스크롤 등)는 `buffer.id` 를 키로 한
//   몸체(`GridBody`)에 산다 — 다른 결과를 넘기면 몸체가 새로 생겨 앞 결과의 것이 하나도 넘어오지 않는다(M3-16 리뷰: 옛 열 순서 ·
//   필터가 다음 결과에 걸렸다). 같은 결과 안에서 열이 늦게 오면(`started` 만 받은 결과) 그 렌더에서 처음 값을 쓴다.
// - NULL 은 `NULL` 을 흐린 색(`grid.null`) + 기울임으로 그리고(색만으로 가르지 않는다 — 10-ui.md §6), 화면 읽기 프로그램에는
//   「NULL 값」 으로 읽힌다(Glide 접근성 표는 셀의 `data` 를 읽는다). 글자 「NULL」 인 값은 보통 글자다.
// - 열 머리: 이름(굵게) · 타입(흐리게) · 정렬 화살표를 직접 그린다(고른 열은 `grid.accent` 위에 `grid.background` 글자). 접근성
//   표의 열 머리 글은 「이름 (타입)」 이다. 머리 클릭은 정렬(오름 → 내림 → 없음), 드래그는 순서,
//   경계 더블클릭은 화면에 보이는 행으로 너비 맞춤(Glide 의 `getCellsForSelection` 측정).
// - 고르기: 드래그 · ⇧+클릭(사각형), 행 번호 칸(행), 열 머리 ⌘+클릭(열). ⌘C 는 `copy` 이벤트에서 TSV 를 쓴다(Glide 의 복사는
//   끈다 — 글은 copy.ts 가 정한다).
// - 색: `grid.*` 토큰(grid-theme.ts). 테마 · 외관이 바뀌면 다시 칠한다.
// - **편집 대기 표지**(테이블 데이터 M3-17 — 쓰지 않는 화면은 그대로다): `cellMark` 가 돌려준 셀은 배경색(diff 토큰을 섞은
//   `colors.marks`) + 셀 앞 표지 글자(`MARK_GLYPHS`) + 접근성 글(「<label>: <값>」)로 그린다 — 색만으로 가르지 않는다(10-ui.md §6).
//   `cellMark` 함수가 바뀌면 다시 칠한다(쓰는 화면이 변경마다 새 함수를 넘긴다). `onCellClick` 은 셀 클릭(⌘ · Ctrl 을 함께 —
//   FK 따라가기), 핸들의 `bounds` 는 셀의 화면 사각형(제자리 편집 칸의 자리)이다.

import "@glideapps/glide-data-grid/dist/index.css";

import {
  CompactSelection,
  DataEditor,
  type DataEditorRef,
  type DrawHeaderCallback,
  type GridCell,
  GridCellKind,
  type GridColumn,
  type GridSelection,
  type Item,
  type Theme,
} from "@glideapps/glide-data-grid";
import type { ColumnMeta, DbValue } from "@pitwall/rpc-client";
import {
  type ClipboardEvent,
  forwardRef,
  useCallback,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";

import { toTsv } from "./copy";
import { type GridCellMarkKind, type GridColors, glideTheme, NULL_FONT_STYLE } from "./grid-theme";
import type { ResultBuffer, SortSpec } from "./result-buffer";
import { EMPTY_PICK, type GridPick, isEmptyPick, pickBlock } from "./selection";
import { useDbGridStrings } from "./strings";
import { displayText, isNumeric } from "./values";

export interface ResultGridHandle {
  /** 지금 고른 셀(없으면 빈 덩어리). */
  pick(): GridPick;
  /** 보이는 열 순서(보이는 → 원래 열 번호). */
  order(): readonly number[];
  focus(): void;
  /** 셀(원래 열 번호, 보이는 행 번호)의 화면 사각형(뷰포트 기준). 열이 숨었거나 그리드가 없으면 `undefined`. */
  bounds(cell: { row: number; column: number }): GridRect | undefined;
}

/** 화면 사각형(뷰포트 기준 px). */
export interface GridRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** 편집 대기 표지 하나: 종류와 접근성 글의 앞말(「바뀜」 등 — 쓰는 화면의 번역). */
export interface GridCellMark {
  kind: GridCellMarkKind;
  label: string;
}

/** 표지 종류마다 셀 앞에 그리는 글자. */
export const MARK_GLYPHS: Record<GridCellMarkKind, string> = {
  changed: "✎",
  inserted: "+",
  deleted: "✕",
};

export interface ResultGridProps {
  buffer: ResultBuffer;
  colors: GridColors;
  /** 셀을 더블클릭 · ↩ 했다(원래 열 번호, 보이는 행 번호). */
  onActivate?: (cell: { row: number; column: number }) => void;
  /** 지금 칸(커서)이 바뀌었다. */
  onCursor?: (cell: { row: number; column: number } | undefined) => void;
  /** 그리드에 포커스가 들어왔다. */
  onFocus?: () => void;
  /** 셀(원래 열 번호, 보이는 행 번호)의 편집 대기 표지. 없으면 표지가 없다. */
  cellMark?: (row: number, column: number) => GridCellMark | undefined;
  /** 셀을 눌렀다(`mod` 는 ⌘ · Ctrl 을 함께 눌렀는가). */
  onCellClick?: (cell: { row: number; column: number }, modifiers: { mod: boolean }) => void;
}

const HEADER_HEIGHT = 28;
const ROW_HEIGHT = 24;

function initialWidth(name: string, type: string): number {
  return Math.max(64, Math.min(320, 7.5 * (name.length + type.length) + 40));
}

function useBufferVersion(buffer: ResultBuffer): number {
  return useSyncExternalStore(
    useCallback((onChange) => buffer.subscribe(onChange), [buffer]),
    () => buffer.version,
  );
}

function toPick(s: GridSelection): GridPick {
  const rects = s.current ? [s.current.range, ...s.current.rangeStack] : [];
  return {
    rects: rects.filter((r) => r.width > 0 && r.height > 0),
    rows: s.rows.toArray(),
    columns: s.columns.toArray(),
  };
}

const EMPTY_SELECTION: GridSelection = {
  columns: CompactSelection.empty(),
  rows: CompactSelection.empty(),
};

/** 결과 그리드. 결과(`buffer.id`)가 바뀌면 몸체를 새로 만든다 — 결과에 묶인 상태가 다음 결과로 넘어가지 않는다. */
export const ResultGrid = forwardRef<ResultGridHandle, ResultGridProps>(
  function ResultGrid(props, ref) {
    return <GridBody key={props.buffer.id} ref={ref} {...props} />;
  },
);

const GridBody = forwardRef<ResultGridHandle, ResultGridProps>(function GridBody(
  { buffer, colors, onActivate, onCursor, onFocus, cellMark, onCellClick },
  ref,
) {
  const T = useDbGridStrings();
  const version = useBufferVersion(buffer);
  const meta = buffer.columns;
  // 열 순서 · 너비 · 고르기는 그 열 목록의 것이다. 결과(버퍼)나 열이 바뀌면 그 렌더에서 곧바로 처음 값을 쓴다 — 효과로 나중에
  // 되돌리면 그 사이 한 번의 렌더가 옛 순서로 새 열(아직 없을 수도 있다)을 읽는다.
  const [layout, setLayout] = useState<{
    meta: readonly ColumnMeta[];
    order: number[];
    widths: Record<number, number>;
    selection: GridSelection;
  }>(() => ({ meta, order: meta.map((_, i) => i), widths: {}, selection: EMPTY_SELECTION }));
  const current =
    layout.meta === meta
      ? layout
      : { meta, order: meta.map((_, i) => i), widths: {}, selection: EMPTY_SELECTION };
  if (current !== layout) setLayout(current);
  const { order, widths, selection } = current;
  const update = useCallback(
    (change: (l: typeof layout) => Partial<typeof layout>) =>
      setLayout((l) => (l.meta === buffer.columns ? { ...l, ...change(l) } : l)),
    [buffer],
  );
  const editor = useRef<DataEditorRef>(null);

  useImperativeHandle(
    ref,
    () => ({
      pick: () => toPick(selection),
      order: () => order,
      focus: () => editor.current?.focus(),
      bounds: ({ row, column }) => {
        const col = order.indexOf(column);
        return col < 0 ? undefined : editor.current?.getBounds(col, row);
      },
    }),
    [selection, order],
  );

  const sort = buffer.sort;
  const columns: GridColumn[] = useMemo(
    () =>
      order.flatMap((src) => {
        const c = meta[src];
        if (!c) return [];
        return {
          id: String(src),
          // 접근성 표의 열 머리 글(그림은 `drawHeader` 가 그린다).
          title: `${c.name} (${c.dbType})`,
          width: widths[src] ?? initialWidth(c.name, c.dbType),
        };
      }),
    [order, meta, widths],
  );

  const viewVersion = buffer.viewVersion;
  // 뷰(정렬 · 필터)가 바뀌면 함수가 바뀌어 Glide 가 다시 칠한다. 행이 붙기만 하면 새 행 번호라 그대로 둔다.
  const getCellContent = useCallback(
    ([col, row]: Item): GridCell => {
      void viewVersion;
      const src = order[col];
      const value: DbValue | undefined = src === undefined ? undefined : buffer.row(row)?.[src];
      if (value === undefined) {
        return { kind: GridCellKind.Loading, allowOverlay: false };
      }
      const text = displayText(value);
      const isNull = value === "null";
      const mark = src === undefined ? undefined : cellMark?.(row, src);
      // 접근성 표가 읽는 글 — NULL 은 「NULL 값」(글자 「NULL」 과 갈린다).
      const spoken = isNull ? T.nullValue : text;
      return {
        kind: GridCellKind.Text,
        data: mark ? `${mark.label}: ${spoken}` : spoken,
        displayData: mark ? `${MARK_GLYPHS[mark.kind]} ${text}` : text,
        allowOverlay: false,
        readonly: true,
        contentAlign: isNumeric(value) ? "right" : "left",
        themeOverride:
          isNull || mark
            ? {
                ...(isNull && { textDark: colors.null, baseFontStyle: NULL_FONT_STYLE }),
                ...(mark && { bgCell: colors.marks[mark.kind] }),
              }
            : undefined,
      };
    },
    [buffer, order, viewVersion, colors.null, colors.marks, T.nullValue, cellMark],
  );

  const drawHeader: DrawHeaderCallback = useCallback(
    ({ ctx, column, rect, theme, isSelected }) => {
      const src = Number(column.id);
      const c = meta[src];
      if (!c) return;
      // 고른 열의 머리는 Glide 가 `accentColor` 로 칠한다 — 그 위 글자는 `textHeaderSelected`(= grid.background).
      const fore = isSelected ? colors.background : colors.headerForeground;
      const type = isSelected ? colors.background : colors.headerType;
      const arrow = isSelected ? colors.background : colors.accent;
      const pad = 8;
      ctx.save();
      ctx.beginPath();
      ctx.rect(rect.x, rect.y, rect.width, rect.height);
      ctx.clip();
      const y = rect.y + rect.height / 2;
      ctx.textBaseline = "middle";
      ctx.font = `${theme.headerFontStyle} ${theme.fontFamily}`;
      ctx.fillStyle = fore;
      ctx.fillText(c.name, rect.x + pad, y);
      const nameWidth = ctx.measureText(c.name).width;
      ctx.font = `${theme.baseFontStyle} ${theme.fontFamily}`;
      ctx.fillStyle = type;
      ctx.fillText(c.dbType, rect.x + pad * 2 + nameWidth, y);
      const s: SortSpec | undefined = sort?.column === src ? sort : undefined;
      if (s) {
        ctx.fillStyle = arrow;
        ctx.textAlign = "right";
        ctx.fillText(s.direction === "asc" ? "▲" : "▼", rect.x + rect.width - pad, y);
      }
      ctx.restore();
    },
    [meta, colors, sort],
  );

  const onHeaderClicked = useCallback(
    (col: number, event: { metaKey: boolean; ctrlKey: boolean; preventDefault: () => void }) => {
      // ⌘(Win/Linux Ctrl)+클릭은 열 고르기(Glide 기본) — 정렬하지 않는다.
      if (event.metaKey || event.ctrlKey) return;
      event.preventDefault();
      const src = order[col];
      if (src === undefined) return;
      const now = buffer.sort;
      const next: SortSpec | undefined =
        now?.column !== src
          ? { column: src, direction: "asc" }
          : now.direction === "asc"
            ? { column: src, direction: "desc" }
            : undefined;
      buffer.setSort(next);
    },
    [buffer, order],
  );

  const onColumnMoved = useCallback(
    (from: number, to: number) =>
      update((l) => {
        const next = [...l.order];
        const [moved] = next.splice(from, 1);
        if (moved !== undefined) next.splice(to, 0, moved);
        return { order: next };
      }),
    [update],
  );

  const onColumnResize = useCallback(
    (column: GridColumn, newSize: number) =>
      update((l) => ({ widths: { ...l.widths, [Number(column.id)]: newSize } })),
    [update],
  );

  const onSelectionChange = useCallback(
    (s: GridSelection) => {
      update(() => ({ selection: s }));
      const cell = s.current?.cell;
      const src = cell ? order[cell[0]] : undefined;
      onCursor?.(cell && src !== undefined ? { row: cell[1], column: src } : undefined);
    },
    [order, onCursor, update],
  );

  const onCellActivated = useCallback(
    ([col, row]: Item) => {
      const src = order[col];
      if (src !== undefined) onActivate?.({ row, column: src });
    },
    [order, onActivate],
  );

  const onCellClicked = useCallback(
    ([col, row]: Item, event: { metaKey: boolean; ctrlKey: boolean }) => {
      const src = order[col];
      if (src !== undefined)
        onCellClick?.({ row, column: src }, { mod: event.metaKey || event.ctrlKey });
    },
    [order, onCellClick],
  );

  const onCopy = useCallback(
    (e: ClipboardEvent<HTMLDivElement>) => {
      const pick = toPick(selection);
      if (isEmptyPick(pick)) return;
      e.preventDefault();
      e.clipboardData.setData("text/plain", toTsv(pickBlock(buffer, order, pick)));
    },
    [selection, buffer, order],
  );

  const theme = useMemo(() => glideTheme(colors) as Partial<Theme>, [colors]);

  return (
    <div
      className="relative min-h-0 flex-1"
      role="region"
      aria-label={T.label}
      data-testid="db-result-grid"
      data-rows={buffer.rowCount}
      data-version={version}
      onCopy={onCopy}
      onFocus={onFocus}
    >
      <DataEditor
        ref={editor}
        columns={columns}
        rows={buffer.rowCount}
        getCellContent={getCellContent}
        getCellsForSelection
        drawHeader={drawHeader}
        theme={theme}
        width="100%"
        height="100%"
        headerHeight={HEADER_HEIGHT}
        rowHeight={ROW_HEIGHT}
        rowMarkers="number"
        smoothScrollX
        smoothScrollY
        rangeSelect="rect"
        columnSelect="multi"
        rowSelect="multi"
        gridSelection={selection}
        onGridSelectionChange={onSelectionChange}
        onHeaderClicked={onHeaderClicked}
        onColumnMoved={onColumnMoved}
        onColumnResize={onColumnResize}
        onCellActivated={onCellActivated}
        onCellClicked={onCellClick ? onCellClicked : undefined}
        keybindings={{ copy: false, paste: false, cut: false, search: false, delete: false }}
        maxColumnAutoWidth={480}
        maxColumnWidth={2000}
      />
    </div>
  );
});

export { EMPTY_PICK };
