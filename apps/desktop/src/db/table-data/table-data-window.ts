// 창 하나의 테이블 데이터 탭들(14-m3-plan.md M3-17): 창이 정한 것(프로젝트 · 미제출 변경 질문)과 열린 탭의 등록부.
//
// - 창의 것은 `DbTableDataFeature`(table-data-feature.tsx)가 둔다 — 탭은 읽기만 한다.
// - 등록부: 탭마다 제출 · 되돌리기 · 행 추가 · 행 삭제 · 닫기를 내놓는다. 창의 액션(⌘↩ · ⌘⌥Z · ⌘N · ⌘⌫)은 키를 받은 순간 포커스가
//   든 탭, 없으면 에디터 영역의 지금 탭에 돌린다(콘솔의 console-window.ts 와 같은 꼴).
// - 이 창에서 사람이 연 탭(`markOpenedHere`)은 곧바로 연결한다 — 연 동작이 곧 누름이다. 되살린 탭은 여기 없다(결정 M3-D13).

import { createStore } from "zustand/vanilla";

export interface DiscardQuestion {
  table: string;
  count: number;
}

interface TableDataWindowState {
  projectId: string | undefined;
  /** 미제출 변경을 버리고 닫을지 묻는다(창에 하나). 버리면 `true`. */
  askDiscard: ((question: DiscardQuestion) => Promise<boolean>) | undefined;
}

export const tableDataWindow = createStore<TableDataWindowState>()(() => ({
  projectId: undefined,
  askDiscard: undefined,
}));

/** 테이블 데이터 탭 하나가 창에 내놓는 것. */
export interface TableDataHandle {
  readonly panelId: string;
  root(): HTMLElement | null;
  submit(): void;
  revert(): void;
  addRow(): void;
  deleteRows(): void;
  /** 편집이 켜졌다. */
  canEdit(): boolean;
  /** 제출하지 않은 변경이 있다. */
  hasPending(): boolean;
  /** 탭이 닫혔다 — 세션을 닫는다. */
  dispose(): void;
}

const tabs = new Map<string, TableDataHandle>();

export function registerTableData(handle: TableDataHandle): () => void {
  tabs.set(handle.panelId, handle);
  return () => {
    if (tabs.get(handle.panelId) === handle) tabs.delete(handle.panelId);
  };
}

export function tableDataOf(panelId: string): TableDataHandle | undefined {
  return tabs.get(panelId);
}

/** 키를 받은 탭: 포커스가 든 탭, 없으면 `activeTabId` 의 탭. */
export function targetTableData(
  activeTabId: string | undefined,
  doc: Document = document,
): TableDataHandle | undefined {
  const focused = doc.activeElement;
  if (focused) for (const t of tabs.values()) if (t.root()?.contains(focused)) return t;
  return activeTabId === undefined ? undefined : tabs.get(activeTabId);
}

const openedHere = new Set<string>();

/** 이 창에서 사람이 연 탭(곧바로 연결한다). */
export function markOpenedHere(tabId: string) {
  openedHere.add(tabId);
}

/**
 * 이 창에서 사람이 연 탭인가 — 쓰는 순간 지운다(곧바로 연결하는 것은 한 번뿐이다: 그 뒤에 탭이 다시 그려지거나 같은 창에서 탭
 * 묶음이 되살아나도 연결하지 않는다).
 */
export function takeOpenedHere(tabId: string): boolean {
  return openedHere.delete(tabId);
}
