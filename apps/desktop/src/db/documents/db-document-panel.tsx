// 에디터 탭 안의 DB 문서(spec.ts): 종류(`kind`)마다 문서 컴포넌트를 고른다. 행마다 `DB_DOCUMENTS` 에 한 줄씩 더한다.
//
// 문서 컴포넌트는 마운트만으로 연결 · 세션 RPC 를 부르지 않는다(spec.ts 머리 주석 — 복원된 탭은 연결하지 않은 채 열린다).

import type { ComponentType } from "react";

import type { EditorTabParams } from "../../editor/editor-tabs";
import { ConsoleTab } from "../console/console-tab";
import { DdlDocument } from "../explorer/ddl-document";
import { RedisDocument } from "../redis/redis-document";
import type { DbDocumentKind, DbDocumentSpec } from "./spec";

export interface DbDocumentProps<K extends DbDocumentKind = DbDocumentKind> {
  spec: Extract<DbDocumentSpec, { kind: K }>;
  params: EditorTabParams;
  /** 탭의 패널 id(dockview). `useDbTabSession(panelId, …)` 의 키다(src/db/safety/db-status.tsx). */
  panelId: string;
}

const DB_DOCUMENTS: { [K in DbDocumentKind]: ComponentType<DbDocumentProps<K>> } = {
  console: ConsoleTab,
  redis: RedisDocument,
  ddl: DdlDocument,
};

export function DbDocumentPanel({ spec, params, panelId }: DbDocumentProps) {
  const Document = DB_DOCUMENTS[spec.kind] as ComponentType<DbDocumentProps>;
  return <Document spec={spec} params={params} panelId={panelId} />;
}
