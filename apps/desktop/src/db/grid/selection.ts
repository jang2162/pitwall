// 그리드에서 고른 셀(범위 · 행 · 열)을 복사할 덩어리로(M3-16). Glide 의 선택 모양을 몰라도 되게 감싸개가 이 모양으로 옮겨 준다.

import type { ColumnMeta, DbValue } from "@pitwall/rpc-client";

import type { CellBlock } from "./copy";
import type { ResultBuffer } from "./result-buffer";

/** 화면 기준(열은 보이는 순서, 행은 정렬 · 필터 뒤 순서)의 사각형. */
export interface CellRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface GridPick {
  /** 고른 사각형(⇧+클릭 · 드래그). 여럿이면 합친 경계 사각형을 쓴다. */
  rects: CellRect[];
  /** 통째로 고른 행(행 번호 칸). */
  rows: number[];
  /** 통째로 고른 열(열 머리). */
  columns: number[];
}

export const EMPTY_PICK: GridPick = { rects: [], rows: [], columns: [] };

export function isEmptyPick(p: GridPick): boolean {
  return p.rects.length === 0 && p.rows.length === 0 && p.columns.length === 0;
}

/**
 * 고른 것을 덩어리로. `order` 는 보이는 열 → 원래 열 번호. 행을 통째로 고르면 모든 열, 열을 통째로 고르면 모든 행이다.
 * 사각형 여럿은 경계 사각형 하나로 합친다(스프레드시트의 「여러 범위는 복사할 수 없다」 대신).
 */
export function pickBlock(
  buffer: ResultBuffer,
  order: readonly number[],
  pick: GridPick,
): CellBlock {
  const meta = buffer.columns;
  let colIdx: number[];
  let rowIdx: number[];
  if (pick.rows.length > 0) {
    rowIdx = [...pick.rows].sort((a, b) => a - b);
    colIdx = order.map((_, i) => i);
  } else if (pick.columns.length > 0) {
    colIdx = [...pick.columns].sort((a, b) => a - b);
    rowIdx = Array.from({ length: buffer.rowCount }, (_, i) => i);
  } else if (pick.rects.length > 0) {
    const x0 = Math.min(...pick.rects.map((r) => r.x));
    const y0 = Math.min(...pick.rects.map((r) => r.y));
    const x1 = Math.max(...pick.rects.map((r) => r.x + r.width));
    const y1 = Math.max(...pick.rects.map((r) => r.y + r.height));
    colIdx = range(x0, x1);
    rowIdx = range(y0, Math.min(y1, buffer.rowCount));
  } else {
    return { columns: [], rows: [] };
  }
  const src = colIdx.map((c) => order[c]).filter((c): c is number => c !== undefined);
  const columns: ColumnMeta[] = src.map((c) => meta[c]!);
  const rows: DbValue[][] = [];
  for (const r of rowIdx) {
    const row = buffer.row(r);
    if (row) rows.push(src.map((c) => row[c] ?? "null"));
  }
  return { columns, rows };
}

function range(from: number, to: number): number[] {
  const out: number[] = [];
  for (let i = from; i < to; i++) out.push(i);
  return out;
}
