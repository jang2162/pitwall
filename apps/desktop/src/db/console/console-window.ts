// 창 하나의 SQL 콘솔들(14-m3-plan.md M3-15): 창이 정한 것(프로젝트 · 열린 트랜잭션 질문)과 열린 콘솔 탭의 등록부.
//
// - 창의 것은 `DbConsoleFeature`(console-feature.tsx)가 둔다 — 콘솔 탭은 읽기만 한다.
// - 등록부: 탭마다 실행 · 취소 · 실행 계획 · 닫기를 내놓는다. 창의 액션(⌘↩ · ⌘F2 …)은 키를 받은 순간 포커스가 든 콘솔, 없으면
//   에디터 영역의 지금 탭의 콘솔에 돌린다(그리드의 grid-focus.ts 와 같은 꼴).
// - 마지막으로 쓴 연결: `db.console.open` 을 인자 없이(⌘⇧L) 부르면 이 창에서 마지막으로 연결한 연결의 새 콘솔이다. 창의 편의
//   값이라 웹뷰 저장소에 두고, 읽고 쓰지 못해도 동작은 같다(연결 없이 열린다).

import { createStore } from "zustand/vanilla";

import type { TransactionAnswer, TransactionQuestion } from "./transaction-dialog";

interface ConsoleWindowState {
  projectId: string | undefined;
  /** 열린 트랜잭션 질문(창에 하나 — 탭 닫기 · 창 닫기 · 앱 종료가 차례로 묻는다). */
  askTransaction: ((question: TransactionQuestion) => Promise<TransactionAnswer>) | undefined;
}

export const consoleWindow = createStore<ConsoleWindowState>()(() => ({
  projectId: undefined,
  askTransaction: undefined,
}));

/** 콘솔 탭 하나가 창에 내놓는 것. */
export interface ConsoleHandle {
  readonly panelId: string;
  /** 탭의 뿌리 요소(포커스가 이 안에 있는가). */
  root(): HTMLElement | null;
  /** 실행(⌘↩) — `all` 이면 「전체 실행」. */
  run(all: boolean): void;
  /** ■ 취소(⌘F2). */
  cancel(): void;
  explain(): void;
  /** 세션이 있고 도는 실행이 없다. */
  canRun(): boolean;
  /** 도는 실행이 있다. */
  canCancel(): boolean;
  /** 탭이 닫혔다 — 글을 마저 쓰고 세션을 닫는다. */
  dispose(): void;
}

const consoles = new Map<string, ConsoleHandle>();

/** 콘솔 탭을 등록한다. 돌려준 함수로 뺀다(같은 탭의 다음 등록이 덮었으면 아무것도 하지 않는다). */
export function registerConsole(handle: ConsoleHandle): () => void {
  consoles.set(handle.panelId, handle);
  return () => {
    if (consoles.get(handle.panelId) === handle) consoles.delete(handle.panelId);
  };
}

export function consoleOf(panelId: string): ConsoleHandle | undefined {
  return consoles.get(panelId);
}

/** 키를 받은 콘솔: 포커스가 든 콘솔, 없으면 `activeTabId` 의 콘솔. */
export function targetConsole(
  activeTabId: string | undefined,
  doc: Document = document,
): ConsoleHandle | undefined {
  const focused = doc.activeElement;
  if (focused) for (const c of consoles.values()) if (c.root()?.contains(focused)) return c;
  return activeTabId === undefined ? undefined : consoles.get(activeTabId);
}

export const LAST_CONNECTION_KEY = "pitwall.db.console.lastConnection";

/** 이 프로젝트에서 마지막으로 연결한 연결 id(웹뷰 저장소 — 읽지 못하면 `undefined`). */
export function lastConnection(projectId: string): string | undefined {
  try {
    const all = JSON.parse(localStorage.getItem(LAST_CONNECTION_KEY) ?? "{}") as unknown;
    const id =
      all && typeof all === "object" ? (all as Record<string, unknown>)[projectId] : undefined;
    return typeof id === "string" ? id : undefined;
  } catch {
    return undefined;
  }
}

export function rememberConnection(projectId: string, connectionId: string) {
  try {
    const raw = JSON.parse(localStorage.getItem(LAST_CONNECTION_KEY) ?? "{}") as unknown;
    const all = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
    localStorage.setItem(
      LAST_CONNECTION_KEY,
      JSON.stringify({ ...all, [projectId]: connectionId }),
    );
  } catch {
    // 저장소가 막혀도 연결은 그대로 된다 — 다음 ⌘⇧L 이 연결 없이 열릴 뿐이다.
  }
}
