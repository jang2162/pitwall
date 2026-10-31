// 지금 포커스가 있는 결과 그리드(M3-16). 액션 `db.grid.copyAs`(⌘⇧C, db-grid-feature.tsx)는 창에 하나만 등록되고, 키를 받은
// 순간 포커스가 든 그리드에 「다른 형식으로 복사」 를 연다. 그리드가 여럿(결과 탭 · 테이블 데이터 · Redis)이어도 같은 액션이다.

export interface GridTarget {
  /** 그리드의 뿌리 요소(포커스가 이 안에 있어야 액션이 산다). */
  root(): HTMLElement | null;
  /** 「다른 형식으로 복사」 고르기를 연다. */
  openCopyAs(): void;
}

let active: GridTarget | undefined;
const listeners = new Set<() => void>();

export function setActiveGrid(target: GridTarget) {
  active = target;
  for (const l of listeners) l();
}

export function clearActiveGrid(target: GridTarget) {
  if (active !== target) return;
  active = undefined;
  for (const l of listeners) l();
}

/** 포커스가 든 그리드. 마지막으로 포커스를 받은 그리드라도 포커스가 지금 그 안에 없으면 없다. */
export function focusedGrid(doc: Document = document): GridTarget | undefined {
  const root = active?.root();
  if (!root || !doc.activeElement || !root.contains(doc.activeElement)) return undefined;
  return active;
}

export function onActiveGridChange(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}
