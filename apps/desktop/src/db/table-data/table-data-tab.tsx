// 테이블 데이터를 DB 문서 탭(src/db/documents — 에디터 영역의 `db` 칸)에 잇는다: 탭의 `db` 칸이 저장되는 것(`TableDataSpec`)이고,
// WHERE · ORDER BY 가 바뀌면 탭 params 를 고쳐 워크트리 탭 묶음에 남긴다(다시 시작해도 같은 조각으로 연결하지 않은 채 열린다 —
// 결정 M3-D13). 새 탭은 `openTableDataTab` 이 연다.

import type { ObjectRef } from "@pitwall/rpc-client";
import { useCallback } from "react";

import { openTab, tabParams } from "../../editor/editor-tabs";
import { type EditorWorkspace, useEditorWorkspace } from "../../editor/editor-workspace";
import type { DbDocumentProps } from "../documents/db-document-panel";
import { type DbDocumentSpec, dbDocumentKey, newDbTabId } from "../documents/spec";
import { TableDataDocument } from "./table-data-document";
import type { TableDataSpec } from "./table-data-store";
import { markOpenedHere } from "./table-data-window";

type TableDataDocumentSpec = Extract<DbDocumentSpec, { kind: "tableData" }>;

export function TableDataTab({ spec, panelId }: DbDocumentProps<"tableData">) {
  const workspace = useEditorWorkspace();
  const onSpec = useCallback(
    (next: TableDataSpec) => {
      const panel = workspace.api()?.getPanel(panelId);
      if (!panel) return;
      const db: TableDataDocumentSpec = { kind: "tableData", ...next };
      panel.api.updateParameters({ ...tabParams(panel), db });
    },
    [workspace, panelId],
  );
  const { kind: _kind, ...table } = spec;
  return <TableDataDocument panelId={panelId} spec={table} onSpec={onSpec} />;
}

const sameObject = (a: ObjectRef, b: ObjectRef) =>
  a.name === b.name && a.schema === b.schema && a.database === b.database;

/**
 * 테이블 데이터 탭을 연다. 같은 연결 · 같은 객체 · 같은 WHERE 의 탭이 있으면 그것을 앞으로(WHERE 를 주지 않았으면 그 객체의 아무
 * 탭). 새 탭은 이 창에서 사람이 연 탭이라 곧바로 연결한다.
 */
export function openTableDataTab(
  workspace: Pick<EditorWorkspace, "api" | "recent" | "focus">,
  args: { connectionId: string; object: ObjectRef; where?: string },
  title: string,
): string | undefined {
  const api = workspace.api();
  if (!api) return undefined;
  const existing = api.panels.find((p) => {
    const db = tabParams(p).db;
    return (
      db?.kind === "tableData" &&
      db.connectionId === args.connectionId &&
      sameObject(db.object, args.object) &&
      (args.where === undefined || (db.where ?? "") === args.where)
    );
  });
  if (existing) {
    existing.api.setActive();
    workspace.focus(existing.id);
    return existing.id;
  }
  const db: TableDataDocumentSpec = {
    kind: "tableData",
    tabId: newDbTabId(),
    connectionId: args.connectionId,
    object: args.object,
    where: args.where ?? "",
    orderBy: "",
  };
  markOpenedHere(db.tabId);
  const { panel } = openTab(api, {
    path: dbDocumentKey(db),
    db,
    title,
    recent: workspace.recent(),
  });
  workspace.focus(panel.id);
  return panel.id;
}
