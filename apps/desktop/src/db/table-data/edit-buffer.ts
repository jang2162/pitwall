// 테이블 데이터의 행 버퍼(14-m3-plan.md M3-17): 결과 그리드(M3-16)의 `ResultBuffer` 를 이어 받아 **제출하지 않은 변경**을 얹는다.
// 그리드는 버퍼의 `row(i)` · `rowCount` 만 읽으므로 고친 셀의 새 값 · 추가 행이 그리드를 고치지 않고 보인다.
//
// - 읽은 행(서버의 결과)은 원래 번호로 가리킨다. 테이블 데이터는 보유 행 위의 정렬 · 필터를 쓰지 않으므로(정렬은 서버의 ORDER BY,
//   필터 막대는 숨긴다) 보이는 번호 = 원래 번호다.
// - 추가 행은 읽은 행 뒤에 붙는다(보이는 번호 `loadedRows + i`). 값을 넣지 않은 칸은 「기본값」 표시(`DbOther`)다 — 제출할 때는
//   보내지 않는다(서버의 기본값).
// - 지울 행은 값을 그대로 두고 표지만 단다(그리드의 `cellMark`). 고친 셀 · 추가 행 · 지울 행의 표지는 `mark(row, column)`.
// - 정렬: 헤더 클릭(`setSort`)은 보유 행을 정렬하지 않고 `onSort` 로 넘긴다 — 화면이 ORDER BY 를 바꿔 다시 읽는다. `sort` 는
//   화면이 알려 준 서버 정렬이다(헤더의 화살표).
// - 바뀌면 `version` · `viewVersion` 을 올리고 구독자에게 알린다(그리드는 행 수 · 함수가 바뀌면 다시 칠한다).

import type { DbValue } from "@pitwall/rpc-client";

import type { GridCellMarkKind } from "../grid/grid-theme";
import { ResultBuffer, type SortSpec } from "../grid/result-buffer";

/** 추가 행에서 값을 넣지 않은 칸(제출하지 않는다 — 서버의 기본값). */
export const DEFAULT_CELL: DbValue = { other: { text: "DEFAULT", typeName: "" } };

export function isDefaultCell(value: DbValue | undefined): boolean {
  return value === DEFAULT_CELL;
}

export class EditBuffer extends ResultBuffer {
  /** 읽은 행의 고친 셀: 원래 행 번호 → (열 → 새 값). */
  private readonly edits = new Map<number, Map<number, DbValue>>();
  /** 추가 행: 열 → 값(없는 열은 기본값). */
  private readonly added: Map<number, DbValue>[] = [];
  /** 지울 행(원래 행 번호). */
  private readonly removed = new Set<number>();
  private serverSort: SortSpec | undefined;
  private readonly own = new Set<() => void>();
  /** 헤더 클릭 정렬 — 화면이 서버 ORDER BY 로 다시 읽는다. */
  onSort: ((sort: SortSpec | undefined) => void) | undefined;
  /** 변경을 바꿀 때마다 오른다(`cellMark` 를 새로 만드는 열쇠). */
  pendingVersion = 0;

  override get rowCount(): number {
    return this.loadedRows + this.added.length;
  }

  override row(i: number): DbValue[] | undefined {
    const loaded = this.loadedRows;
    if (i >= loaded) {
      const added = this.added[i - loaded];
      if (!added) return undefined;
      return this.columns.map((_, c) => added.get(c) ?? DEFAULT_CELL);
    }
    const base = super.row(i);
    const edits = base && this.edits.get(i);
    if (!base || !edits) return base;
    return base.map((v, c) => (edits.has(c) ? edits.get(c)! : v));
  }

  override get sort(): SortSpec | undefined {
    return this.serverSort;
  }

  override setSort(sort: SortSpec | undefined) {
    this.onSort?.(sort);
  }

  /** 화면이 서버 정렬을 알린다(헤더 화살표). */
  showSort(sort: SortSpec | undefined) {
    this.serverSort = sort;
    this.touch();
  }

  override subscribe(listener: () => void): () => void {
    const off = super.subscribe(listener);
    this.own.add(listener);
    return () => {
      off();
      this.own.delete(listener);
    };
  }

  override dispose() {
    super.dispose();
    this.own.clear();
  }

  /** 읽은 행의 원래 값(고치기 전). 추가 행이면 `undefined`. */
  original(row: number): DbValue[] | undefined {
    return row < this.loadedRows ? super.row(row) : undefined;
  }

  /** 보이는 행이 추가 행인가. */
  isAdded(row: number): boolean {
    return row >= this.loadedRows && row < this.rowCount;
  }

  isRemoved(row: number): boolean {
    return this.removed.has(row);
  }

  /** 셀 하나를 고친다. 읽은 행이면 원래 값과 같아지면 고침을 지운다. 지울 행은 고치지 않는다. */
  setCell(row: number, column: number, value: DbValue, same: (a: DbValue, b: DbValue) => boolean) {
    if (this.isAdded(row)) {
      const added = this.added[row - this.loadedRows]!;
      added.set(column, value);
    } else {
      const base = super.row(row);
      if (!base || this.removed.has(row)) return;
      const edits = this.edits.get(row) ?? new Map<number, DbValue>();
      if (same(base[column]!, value)) edits.delete(column);
      else edits.set(column, value);
      if (edits.size === 0) this.edits.delete(row);
      else this.edits.set(row, edits);
    }
    this.touch();
  }

  /** 빈 행을 뒤에 붙인다. 그 행의 보이는 번호. */
  addRow(): number {
    this.added.push(new Map());
    this.touch();
    return this.rowCount - 1;
  }

  /** 행들을 지운다: 추가 행은 없애고, 읽은 행은 「지울 행」 으로 둔다(이미 지울 행이면 되살린다). */
  toggleRemove(rows: readonly number[]) {
    const loaded = this.loadedRows;
    const addedOut = rows
      .filter((r) => r >= loaded && r < this.rowCount)
      .map((r) => r - loaded)
      .sort((a, b) => b - a);
    for (const i of addedOut) this.added.splice(i, 1);
    const read = rows.filter((r) => r < loaded);
    const allRemoved = read.length > 0 && read.every((r) => this.removed.has(r));
    for (const r of read) {
      if (allRemoved) this.removed.delete(r);
      else this.removed.add(r);
    }
    this.touch();
  }

  /** 모든 변경을 버린다. */
  revert() {
    this.edits.clear();
    this.added.length = 0;
    this.removed.clear();
    this.touch();
  }

  /** 변경의 수(고친 행 · 추가 행 · 지울 행 — 지울 행의 고침은 세지 않는다). */
  get pendingCount(): number {
    let n = this.added.length + this.removed.size;
    for (const row of this.edits.keys()) if (!this.removed.has(row)) n++;
    return n;
  }

  /** 제출할 것: 고친 행(지울 행 제외) · 추가 행 · 지울 행. */
  pendingChanges(): {
    updates: { row: number; values: Map<number, DbValue> }[];
    inserts: Map<number, DbValue>[];
    deletes: number[];
  } {
    return {
      updates: [...this.edits.entries()]
        .filter(([row]) => !this.removed.has(row))
        .sort(([a], [b]) => a - b)
        .map(([row, values]) => ({ row, values })),
      inserts: [...this.added],
      deletes: [...this.removed].sort((a, b) => a - b),
    };
  }

  /** 셀의 표지. 지울 행이 가장 앞선다. */
  mark(row: number, column: number): GridCellMarkKind | undefined {
    if (this.removed.has(row)) return "deleted";
    if (this.isAdded(row)) return "inserted";
    return this.edits.get(row)?.has(column) ? "changed" : undefined;
  }

  private touch() {
    this.pendingVersion++;
    this.version++;
    this.viewVersion++;
    for (const l of Array.from(this.own)) l();
  }
}
