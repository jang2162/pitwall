// 테이블 데이터의 창 단위(14-m3-plan.md M3-17): 프로젝트 창에 한 번 둔다.
//
// - 액션 `db.tableData.open { connectionId, object: ObjectRef, where? }` — DB 탐색기(M3-13)가 테이블 · 뷰 더블클릭 · 우클릭
//   「테이블 데이터 열기」 에서 `{ connectionId, object }` 로 부르고, FK ⌘+클릭이 참조 테이블을 그 키의 WHERE 로 부른다.
// - 편집 액션: `db.tableData.submit`(⌘↩) · `revert`(⌘⌥Z) · `addRow`(⌘N, Win/Linux Alt+Insert) · `deleteRow`(⌘⌫, Win/Linux
//   Ctrl+Y) — 키는 `when = "dbTableDataFocus"`(테이블 데이터 뿌리 · 그 안의 그리드), 키를 받은 순간 포커스가 든 탭, 없으면 에디터
//   영역의 지금 탭(table-data-window.ts).
// - 미제출 변경 질문(창에 하나 — M2-20 의 확인 부품): 탭 닫기 · 창 닫기 · 앱 종료가 작업 공간의 `guardClose`(M3-15 의 틀)로
//   차례로 묻는다.
// - 탭이 닫히면(도킹 영역에서 패널이 빠지면) 그 탭의 세션을 닫는다.

import type { Action } from "@pitwall/keymap";
import type { ObjectRef } from "@pitwall/rpc-client";
import type { DockviewApi } from "dockview-react";
import { useEffect, useMemo } from "react";

import type { EditorWorkspace } from "../../editor/editor-workspace";
import { useGitDialogs } from "../../git/dialogs/git-dialogs";
import { useRegisterActions } from "../../keymap/keymap-provider";
import { type DbTableDataStrings, useDbTableDataStrings } from "./strings";
import { openTableDataTab } from "./table-data-tab";
import { tableDataOf, tableDataWindow, targetTableData } from "./table-data-window";

/** `db.tableData.open` 의 인자. */
export interface TableDataOpenArgs {
  connectionId: string;
  object: ObjectRef;
  where?: string;
}

function isObjectRef(v: unknown): v is ObjectRef {
  if (!v || typeof v !== "object") return false;
  const o = v as Record<string, unknown>;
  const part = (k: string) => o[k] === null || typeof o[k] === "string";
  return typeof o.name === "string" && o.name !== "" && part("database") && part("schema");
}

/** 액션 인자를 읽는다. 꼴이 틀리면 `undefined`(아무것도 열지 않는다). */
export function openArgs(args: unknown): TableDataOpenArgs | undefined {
  if (!args || typeof args !== "object") return undefined;
  const { connectionId, object, where } = args as Record<string, unknown>;
  if (typeof connectionId !== "string" || connectionId === "" || !isObjectRef(object))
    return undefined;
  return {
    connectionId,
    object: { database: object.database, schema: object.schema, name: object.name },
    ...(typeof where === "string" && { where }),
  };
}

export function tableDataActions(deps: {
  strings: DbTableDataStrings["actions"];
  activeTab(): string | undefined;
  open(args: TableDataOpenArgs): void;
}): Action[] {
  const T = deps.strings;
  const target = () => targetTableData(deps.activeTab());
  return [
    {
      id: "db.tableData.open",
      label: T.open,
      keywords: ["Table Data", "Open Table", "Browse Data"],
      run: (args) => {
        const parsed = openArgs(args);
        if (parsed) deps.open(parsed);
      },
    },
    {
      id: "db.tableData.submit",
      label: T.submit,
      keywords: ["Submit", "Commit Changes", "Apply Changes"],
      enabled: () => target()?.hasPending() === true,
      run: () => target()?.submit(),
    },
    {
      id: "db.tableData.revert",
      label: T.revert,
      keywords: ["Revert", "Discard Changes"],
      enabled: () => target()?.hasPending() === true,
      run: () => target()?.revert(),
    },
    {
      id: "db.tableData.addRow",
      label: T.addRow,
      keywords: ["Add Row", "Insert Row", "New Row"],
      enabled: () => target()?.canEdit() === true,
      run: () => target()?.addRow(),
    },
    {
      id: "db.tableData.deleteRow",
      label: T.deleteRow,
      keywords: ["Delete Row", "Remove Row"],
      enabled: () => target()?.canEdit() === true,
      run: () => target()?.deleteRows(),
    },
  ];
}

export function DbTableDataFeature({
  projectId,
  workspace,
  api,
}: {
  projectId: string;
  workspace: Pick<EditorWorkspace, "activeTab" | "api" | "recent" | "focus">;
  api: Pick<DockviewApi, "onDidRemovePanel"> | undefined;
}) {
  const T = useDbTableDataStrings();
  const dialogs = useGitDialogs();
  const { confirm } = dialogs;

  useEffect(() => {
    tableDataWindow.setState({
      projectId,
      askDiscard: ({ table, count }) =>
        confirm({
          title: T.close.title,
          body: T.close.body(table, count),
          confirm: T.close.confirm,
          danger: true,
        }),
    });
    return () => tableDataWindow.setState({ projectId: undefined, askDiscard: undefined });
  }, [projectId, confirm, T]);

  useEffect(() => {
    const removed = api?.onDidRemovePanel((panel) => tableDataOf(panel.id)?.dispose());
    return () => removed?.dispose();
  }, [api]);

  const actions = useMemo(
    () =>
      tableDataActions({
        strings: T.actions,
        activeTab: () => workspace.activeTab()?.id,
        open: (args) => void openTableDataTab(workspace, args, T.tabTitle(args.object.name)),
      }),
    [T, workspace],
  );
  useRegisterActions(actions);

  return dialogs.element ?? null;
}
