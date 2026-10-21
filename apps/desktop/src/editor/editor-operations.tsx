// 창에 에디터 조작(12-m1-plan M1-18)을 붙인다: 키맵에 조작 액션(editor-ops.ts)을, 상태바에 에디터 항목(editor-status.tsx)을.
// 줄로 이동 대화상자(goto-line.tsx)는 돌려준 `element` 로 창이 그린다.
// 줄 주석 기호는 `lang.list` 에서 받는다 — 앱이 시작할 때 읽은 확장 레지스트리 것이라 다시 부르지 않는다.

import type { DockviewApi } from "dockview-react";
import { type ReactNode, useCallback, useMemo, useRef } from "react";

import { useRegisterActions } from "../keymap/keymap-provider";
import { useProjectStrings } from "../layout/strings";
import { useRpcQuery } from "../rpc/host";
import { editorOpsActions } from "./editor-ops";
import { useEditorStatusItems } from "./editor-status";
import type { EditorWorkspace } from "./editor-workspace";
import { useGotoLine } from "./goto-line";

export function useEditorOperations(
  workspace: EditorWorkspace,
  api: DockviewApi | undefined,
): ReactNode {
  const strings = useProjectStrings().editor.actions;
  const list = useRpcQuery("lang.list", null, { staleTime: Infinity });
  const comments = useRef(new Map<string, string>());
  comments.current = useMemo(
    () =>
      new Map(
        (list.data?.languages ?? []).flatMap((l) =>
          l.lineComment ? [[l.name, l.lineComment] as const] : [],
        ),
      ),
    [list.data],
  );
  const goto = useGotoLine();
  const actions = useMemo(
    () =>
      editorOpsActions({
        workspace,
        strings,
        openGotoLine: goto.open,
        lineComment: (language) => comments.current.get(language),
      }),
    [workspace, strings, goto.open],
  );
  useRegisterActions(actions);
  const openGotoLine = useCallback(() => {
    const panel = workspace.activeTab();
    const view = panel && workspace.view(panel.id);
    if (view) goto.open(view);
  }, [workspace, goto.open]);
  useEditorStatusItems(workspace, api, openGotoLine);
  return goto.element;
}
