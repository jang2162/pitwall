// SQL 콘솔을 DB 문서 탭(src/db/documents — 에디터 영역의 `db` 칸)에 잇는다: 탭의 `db` 칸이 콘솔의 저장되는 것(`ConsoleSpec`)이고,
// 콘솔이 연결 · DB · 스키마를 바꾸면 탭 params 를 고쳐 워크트리 탭 묶음에 남긴다(다시 시작해도 같은 `tabId` 로 연결하지 않은 채
// 열린다 — 결정 M3-D13). 새 콘솔 탭은 `openConsoleTab` 이 연다.

import { useCallback } from "react";

import { openTab, tabParams } from "../../editor/editor-tabs";
import { type EditorWorkspace, useEditorWorkspace } from "../../editor/editor-workspace";
import type { DbDocumentProps } from "../documents/db-document-panel";
import { type DbDocumentSpec, dbDocumentKey, newDbTabId } from "../documents/spec";
import { ConsoleDocument } from "./console-document";
import type { ConsoleSpec } from "./console-store";

type ConsoleDocumentSpec = Extract<DbDocumentSpec, { kind: "console" }>;

export function ConsoleTab({ spec, panelId }: DbDocumentProps<"console">) {
  const workspace = useEditorWorkspace();
  const onSpec = useCallback(
    (next: ConsoleSpec) => {
      const panel = workspace.api()?.getPanel(panelId);
      if (!panel) return;
      const db: ConsoleDocumentSpec = { kind: "console", ...next };
      panel.api.updateParameters({ ...tabParams(panel), db });
    },
    [workspace, panelId],
  );
  const onTitle = useCallback(
    (title: string) => {
      const panel = workspace.api()?.getPanel(panelId);
      if (panel && panel.title !== title) panel.api.setTitle(title);
    },
    [workspace, panelId],
  );
  const { kind: _kind, ...console } = spec;
  return <ConsoleDocument panelId={panelId} spec={console} onSpec={onSpec} onTitle={onTitle} />;
}

/** 새 콘솔 탭을 연다(`db.console.open`). 콘솔 id 는 탭 id 에서 만든다 — 영소문자 · 숫자 · `-`(콘솔 파일 이름 규칙). */
export function openConsoleTab(
  workspace: Pick<EditorWorkspace, "api" | "recent" | "focus">,
  choice: { connectionId: string | null; database?: string | null },
  title: string,
): string | undefined {
  const api = workspace.api();
  if (!api) return undefined;
  const tabId = newDbTabId();
  const db: ConsoleDocumentSpec = {
    kind: "console",
    tabId,
    consoleId: `c-${tabId.toLowerCase()}`,
    connectionId: choice.connectionId,
    database: choice.database ?? null,
    schema: null,
  };
  const { panel } = openTab(api, {
    path: dbDocumentKey(db),
    db,
    title,
    recent: workspace.recent(),
  });
  workspace.focus(panel.id);
  return panel.id;
}
