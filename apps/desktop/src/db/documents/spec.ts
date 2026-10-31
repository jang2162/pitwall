// 에디터 영역의 DB 문서(SQL 콘솔 M3-15 · 테이블 데이터 M3-17 · Redis 키 M3-18 — 10-ui.md 「에디터 영역 문서 타입」)의 꼴. 에디터
// 탭(src/editor/editor-tabs.ts `EDITOR_PANEL`)의 params `db` 칸이 이것이다. 이 칸이 있는 탭은 파일 문서가 아니다: 문서 모델을 잡지
// 않고(`fs.*` 를 읽지 않는다), 경로를 쓰는 탭 동작(경로 복사 · 저장 · 뷰어 바꾸기 · 외부 도구 문맥 · 최근 파일)에서 빠진다.
//
// git 문서와 다른 것: **워크트리 탭 묶음에 담아 다시 시작하면 되살린다**(src/layout/worktree-session.ts). 되살린 탭은 연결하지
// 않은 채 열린다(결정 M3-D13 — 탭 복원은 「프로젝트를 열 때」 와 같은 순간이다). 그래서 문서 컴포넌트는 마운트만으로 연결 ·
// 세션 RPC(`db.session.open` · `db.kv.*` · `db.query.*`)를 부르지 않는다 — 사람이 [연결] 을 누를 때만 연다.
//
// `tabId` 는 탭을 처음 만들 때 정하고(`newDbTabId`) 복원해도 그대로다 — 콘솔은 그 값을 `db.session.open { tabId }` ·
// `db.session.resume` 에 실어 Host 가 보존한 세션을 다시 찾는다(14-m3-plan M3-15). 탭의 `path` 는 문서 키(`dbDocumentKey`)다 —
// `db:` 로 시작해 실제 경로(절대 경로) · git 문서 키(`git:`)와 겹치지 않는다.
//
// 종류(`kind`)는 행마다 한 줄씩 더한다: 이 유니온, `DB_DOCUMENT_SHAPES`(저장본 검사), db-document-panel.tsx 의 `DB_DOCUMENTS`.
// 있는 종류의 칸 이름 · 뜻을 바꾸지 않는다(저장본이 그 꼴로 남아 있다).

import type { ObjectRef } from "@pitwall/rpc-client";

export type DbDocumentSpec =
  /**
   * Redis 키 문서(M3-18): 왼쪽 키 트리 + 오른쪽 값 뷰어 + 명령 콘솔. `database` 는 DB 번호(`"0"` …) — 위 막대의 번호 전환이 이 값을
   * 바꾼다(그 번호의 연결로 옮긴다).
   */
  | { kind: "redis"; tabId: string; connectionId: string; database: string }
  /**
   * 객체의 DDL(M3-13 — 탐색기의 `⌘B`): 읽기 전용 SQL. 내용(`db.schema.ddl`)은 연 창의 메모리에만 있다 — 되살린 탭은 사람이 [읽기] 를
   * 누를 때 읽는다(메타데이터용 연결을 연다).
   */
  | { kind: "ddl"; tabId: string; connectionId: string; object: ObjectRef };

export type DbDocumentKind = DbDocumentSpec["kind"];

/** 탭 id 의 길이 상한(`DbSessionOpenParams.tabId` 의 1~256자). */
const MAX_TAB_ID = 256;

/** 새 탭 id. 탭을 만들 때 한 번 — 복원은 저장된 값을 쓴다. */
export function newDbTabId(): string {
  return crypto.randomUUID();
}

/** 탭의 `path` 자리에 두는 문서 키. */
export function dbDocumentKey(spec: DbDocumentSpec): string {
  return `db:${spec.kind}:${spec.tabId}`;
}

/** 사람이 읽는 문서 설명(탭 전환기의 둘째 칸 · 탭 툴팁 — 문서 키 `db:…` 를 보이지 않는다). */
export function dbDocumentDetail(spec: DbDocumentSpec): string {
  switch (spec.kind) {
    case "redis":
      return `${spec.connectionId} · DB ${spec.database}`;
    case "ddl":
      return `${spec.connectionId} · ${[spec.object.database, spec.object.schema, spec.object.name].filter(Boolean).join(".")}`;
  }
}

const str = (v: Record<string, unknown>, k: string) => typeof v[k] === "string" && v[k] !== "";

/** 종류마다 저장본 · 탭 params 의 칸 검사(`tabId` · `kind` 밖의 것). */
const DB_DOCUMENT_SHAPES: { [K in DbDocumentKind]: (v: Record<string, unknown>) => boolean } = {
  redis: (v) =>
    str(v, "connectionId") && typeof v.database === "string" && /^\d+$/.test(v.database),
  ddl: (v) => str(v, "connectionId") && isObjectRef(v.object),
};

/** `ObjectRef` 의 꼴(이름은 비지 않고, DB · 스키마는 글 또는 `null`). */
function isObjectRef(value: unknown): boolean {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const o = value as Record<string, unknown>;
  const part = (k: string) => o[k] === null || typeof o[k] === "string";
  return str(o, "name") && part("database") && part("schema");
}

/** 탭 params · 저장본의 `db` 칸이 문서의 꼴인가(옛 · 깨진 값을 거른다). */
export function isDbDocumentSpec(value: unknown): value is DbDocumentSpec {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const v = value as Record<string, unknown>;
  if (typeof v.tabId !== "string" || v.tabId === "" || v.tabId.length > MAX_TAB_ID) return false;
  const shape = Object.hasOwn(DB_DOCUMENT_SHAPES, v.kind as string)
    ? DB_DOCUMENT_SHAPES[v.kind as DbDocumentKind]
    : undefined;
  return shape !== undefined && shape(v);
}
