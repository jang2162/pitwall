// 결과 하나(문장 하나)의 행 버퍼(M3-16). 스트림 청크를 React 상태 밖에 붙이고(10-ui.md 「성능 규칙」), 그리드는 화면 범위의
// 행만 `row(i)` 로 읽는다. 화면은 `subscribe` 로 「바뀌었다」 만 받는다 — 알림은 한 프레임에 한 번으로 모은다(청크마다 다시
// 그리지 않게).
//
// 정렬 · 필터는 보유한 행 위에서 한다(서버로 다시 묻지 않는다 — 서버 정렬 · 필터는 M3-17). 뷰는 원래 행 번호의 배열이다.
// 정렬 · 필터 중에 행이 더 오면: 필터는 새 행만 걸러 뒤에 붙이고, 정렬은 `RESORT_MS` 마다 한 번 다시 정렬한다(그 사이 새 행은
// 정렬되지 않은 채 뒤에 있다). 스트림이 끝나면 곧바로 다시 정렬한다.

import type { ColumnMeta, DbValue } from "@pitwall/rpc-client";

import { compareKeys, displayText, type SortKey, sortKey } from "./values";

export interface SortSpec {
  /** 원래 열 번호. */
  column: number;
  direction: "asc" | "desc";
}

export interface FilterSpec {
  text: string;
  /** 원래 열 번호. 없으면 모든 열. */
  column?: number;
}

export type ResultState =
  | { kind: "streaming" }
  | { kind: "done"; truncated: boolean; elapsedMs: number; rowsAffected: number | null }
  | { kind: "error"; message: string; cancelled: boolean; elapsedMs: number };

/** 정렬 중 행이 더 올 때 다시 정렬하는 간격. */
export const RESORT_MS = 500;

type Schedule = (run: () => void) => void;

const frame: Schedule = (run) => {
  if (typeof requestAnimationFrame === "function") requestAnimationFrame(() => run());
  else setTimeout(run, 0);
};

export class ResultBuffer {
  private cols: ColumnMeta[] = [];
  private readonly rows: DbValue[][] = [];
  private view: number[] | undefined;
  private sortSpec: SortSpec | undefined;
  private filterSpec: FilterSpec | undefined;
  private filteredUpTo = 0;
  private sortedUpTo = 0;
  private resortTimer: ReturnType<typeof setTimeout> | undefined;
  private stateValue: ResultState = { kind: "streaming" };
  private readonly listeners = new Set<() => void>();
  private pending = false;
  /** 바뀔 때마다 오른다 — 화면이 메모 키로 쓴다. 뷰(정렬 · 필터)가 바뀌면 `viewVersion` 도 오른다. */
  version = 0;
  viewVersion = 0;
  /** 첫 행이 들어온 시각(`performance.now()`). 측정용. */
  firstRowAt: number | undefined;

  constructor(private readonly schedule: Schedule = frame) {}

  get columns(): readonly ColumnMeta[] {
    return this.cols;
  }

  get state(): ResultState {
    return this.stateValue;
  }

  get sort(): SortSpec | undefined {
    return this.sortSpec;
  }

  get filter(): FilterSpec | undefined {
    return this.filterSpec;
  }

  /** 받은 행 수(정렬 · 필터와 상관없이). */
  get loadedRows(): number {
    return this.rows.length;
  }

  /** 지금 보이는 행 수(필터 뒤). */
  get rowCount(): number {
    return this.view ? this.view.length : this.rows.length;
  }

  /** 보이는 `i` 번째 행. */
  row(i: number): DbValue[] | undefined {
    const index = this.view ? this.view[i] : i;
    return index === undefined ? undefined : this.rows[index];
  }

  /** 보이는 `i` 번째 행의 원래 번호. */
  sourceIndex(i: number): number | undefined {
    return this.view ? this.view[i] : i < this.rows.length ? i : undefined;
  }

  setColumns(columns: ColumnMeta[]) {
    this.cols = columns;
    this.changed(true);
  }

  append(rows: DbValue[][]) {
    if (rows.length === 0) return;
    if (this.firstRowAt === undefined) this.firstRowAt = performance.now();
    for (const r of rows) this.rows.push(r);
    if (this.filterSpec && !this.sortSpec) this.extendFilter();
    if (this.sortSpec) this.scheduleResort();
    this.changed(false);
  }

  finish(state: Exclude<ResultState, { kind: "streaming" }>) {
    this.stateValue = state;
    if (this.sortSpec && this.sortedUpTo < this.rows.length) this.rebuildView();
    this.changed(true);
  }

  setSort(sort: SortSpec | undefined) {
    this.sortSpec = sort;
    this.rebuildView();
    this.changed(true);
  }

  setFilter(filter: FilterSpec | undefined) {
    this.filterSpec = filter && filter.text !== "" ? filter : undefined;
    this.rebuildView();
    this.changed(true);
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  dispose() {
    if (this.resortTimer !== undefined) clearTimeout(this.resortTimer);
    this.listeners.clear();
  }

  private matches(row: DbValue[]): boolean {
    const f = this.filterSpec;
    if (!f) return true;
    const needle = f.text.toLowerCase();
    if (f.column !== undefined) {
      const v = row[f.column];
      return v !== undefined && displayText(v).toLowerCase().includes(needle);
    }
    return row.some((v) => displayText(v).toLowerCase().includes(needle));
  }

  private extendFilter() {
    const view = (this.view ??= []);
    for (let i = this.filteredUpTo; i < this.rows.length; i++) {
      if (this.matches(this.rows[i]!)) view.push(i);
    }
    this.filteredUpTo = this.rows.length;
  }

  private rebuildView() {
    if (this.resortTimer !== undefined) {
      clearTimeout(this.resortTimer);
      this.resortTimer = undefined;
    }
    if (!this.sortSpec && !this.filterSpec) {
      this.view = undefined;
      this.viewVersion++;
      return;
    }
    const view: number[] = [];
    for (let i = 0; i < this.rows.length; i++) {
      if (this.matches(this.rows[i]!)) view.push(i);
    }
    this.filteredUpTo = this.rows.length;
    const sort = this.sortSpec;
    if (sort) {
      const keys: SortKey[] = Array.from({ length: this.rows.length });
      for (const i of view) {
        const v = this.rows[i]![sort.column];
        keys[i] = v === undefined ? null : sortKey(v);
      }
      const sign = sort.direction === "asc" ? 1 : -1;
      view.sort((a, b) => {
        const ka = keys[a]!;
        const kb = keys[b]!;
        // NULL 은 방향과 상관없이 뒤.
        if (ka === null || kb === null) return compareKeys(ka, kb) || a - b;
        return sign * compareKeys(ka, kb) || a - b;
      });
      this.sortedUpTo = this.rows.length;
    }
    this.view = view;
    this.viewVersion++;
  }

  private scheduleResort() {
    // 다시 정렬하기 전까지 새 행은 정렬 안 된 채 뒤에 붙는다(필터는 거른다).
    const view = (this.view ??= []);
    for (let i = this.filteredUpTo; i < this.rows.length; i++) {
      if (this.matches(this.rows[i]!)) view.push(i);
    }
    this.filteredUpTo = this.rows.length;
    if (this.resortTimer !== undefined) return;
    this.resortTimer = setTimeout(() => {
      this.resortTimer = undefined;
      if (!this.sortSpec) return;
      this.rebuildView();
      this.changed(true);
    }, RESORT_MS);
  }

  private changed(view: boolean) {
    this.version++;
    if (view) this.viewVersion++;
    if (this.pending) return;
    this.pending = true;
    this.schedule(() => {
      this.pending = false;
      for (const l of this.listeners) l();
    });
  }
}
