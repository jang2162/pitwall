// 탐색기가 부르는 · 등록하는 액션 id(14-m3-plan.md M3-13). 다른 행이 등록하는 것은 id 로만 부르고, 등록되지 않았으면 그 항목을
// 끈다(`registry.isAvailable`) — 같은 묶음의 행이 서로 기다리지 않는다.

/** 다른 행이 등록한다. 인자 모양은 14 의 그 행 칸. */
export const EXTERNAL_ACTIONS = {
  /** M3-15 — `{ connectionId?, database? }`. */
  console: "db.console.open",
  /** M3-17 — `{ connectionId, object: ObjectRef }`. */
  tableData: "db.tableData.open",
  /** M3-18 — `{ connectionId, database? }`. */
  redis: "db.redis.open",
  /** M3-14 — 인자 없음. */
  addConnection: "db.connection.add",
  /** M3-14 — `{ connectionId }`. */
  editConnection: "db.connection.edit",
} as const;

/** 이 행이 등록한다. */
export const EXPLORER_ACTIONS = {
  /** `⌘B` — 고른 테이블 · 뷰의 DDL(읽기 전용 SQL 문서). 탐색기에 포커스가 있을 때(`dbExplorerFocus`). */
  ddl: "db.explorer.ddl",
  /**
   * 객체를 탐색기에서 찾아 고르고 포커스를 둔다 — `{ connectionId, object: ObjectRef, kind: "table" | "view" }`, 키 없음. Search
   * Everywhere 의 「DB 객체」 탭(M3-20)이 툴 윈도우를 띄운 뒤 부른다.
   */
  reveal: "db.explorer.reveal",
} as const;

/** 탐색기 트리의 키맵 컨텍스트 키(`data-keymap-context`). */
export const EXPLORER_CONTEXT = "dbExplorerFocus";
