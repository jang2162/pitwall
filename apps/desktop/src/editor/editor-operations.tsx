// 창에 에디터 조작(12-m1-plan M1-18)을 붙인다: 키맵에 조작 액션(editor-ops.ts)을, 상태바에 에디터 항목(editor-status.tsx)을.
// 줄로 이동 대화상자(goto-line.tsx)와 내비게이션 기록의 「최근 위치」 팝업(navigation-ui.tsx)은 돌려준 `element` 로 창이 그린다.
// 줄로 이동은 위치로 가기(`navigateTo`)로 옮긴다 — 떠나는 자리가 뒤로 가기 기록에 든다(M4-09).
// rename · 코드 액션 · 일괄 편집의 액션 · 미리보기(M4-12)는 src/lsp/edit-ui.tsx 가 붙인다.
// 줄 주석 기호는 `lang.list` 에서 받는다 — 앱이 시작할 때 읽은 확장 레지스트리 것이라 다시 부르지 않는다.

import type { DockviewApi } from "dockview-react";
import { type ReactNode, useCallback, useMemo, useRef } from "react";

import { useRegisterActions } from "../keymap/keymap-provider";
import { useProjectStrings } from "../layout/strings";
import { useNotify } from "../notifications/notifications";
import { useRpcQuery } from "../rpc/host";
import { editorOpsActions } from "./editor-ops";
import { useEditorStatusItems } from "./editor-status";
import { type EditorWorkspace, placeCursor } from "./editor-workspace";
import { type GotoLineGo, useGotoLine } from "./goto-line";
import { useNavigationUi } from "./navigation-ui";
import { useLspEditUi } from "../lsp/edit-ui";

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
  const go = useCallback<GotoLineGo>(
    (view, target) => {
      const here = workspace.activeContext();
      // 활성 탭의 뷰가 아니면(있을 수 없지만) 기록 없이 커서만 둔다.
      const panel = workspace.activeTab();
      if (!here?.file || !panel || workspace.view(panel.id) !== view) {
        placeCursor(view, target);
        view.focus();
        return;
      }
      void workspace.navigateTo(
        {
          path: here.file,
          ...(here.worktree !== undefined && { worktree: here.worktree }),
          ...target,
        },
        { record: true },
      );
    },
    [workspace],
  );
  const goto = useGotoLine(go);
  const navigation = useNavigationUi(workspace);
  const lspEdit = useLspEditUi(workspace);
  const notify = useNotify();
  const actions = useMemo(
    () =>
      editorOpsActions({
        workspace,
        strings,
        openGotoLine: goto.open,
        lineComment: (language) => comments.current.get(language),
        notify,
      }),
    [workspace, strings, goto.open, notify],
  );
  useRegisterActions(actions);
  const openGotoLine = useCallback(() => {
    const panel = workspace.activeTab();
    const view = panel && workspace.view(panel.id);
    if (view) goto.open(view);
  }, [workspace, goto.open]);
  useEditorStatusItems(workspace, api, openGotoLine);
  return (
    <>
      {goto.element}
      {navigation}
      {lspEdit}
    </>
  );
}
