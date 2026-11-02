// 창에 rename · 코드 액션 · 일괄 편집(15-m4-plan M4-12)을 붙인다: 창의 편집 문맥(edit-window.ts — 적용기 · 서버 명령 흐름 · 알림)을
// 만들고, 키맵에 액션을 건다 — `lsp.rename`(`⇧F6`, 에디터 포커스 — 파일 트리의 `fileTree.rename` 과 컨텍스트로 갈린다) ·
// `lsp.codeActions`(`⌥↩`) · `lsp.undoLastEdit`(「마지막 일괄 편집 되돌리기」 — 기본 키 없음, 액션 찾기 · 알림의 [되돌리기]). 미리보기
// 대화상자(edit-preview.tsx)는 돌려준 `element` 로 창이 그린다(editor-operations.tsx).
//
// 파일 RPC(`fs.*`)는 창의 Host 연결(`workspace.lsp.services.call`)로, 접근 범위는 활성 탭의 워크트리다. 설정 `[lsp] lightbulb`
// (기본 켬)는 전역 `settings.toml` 에서 읽는다(설정이 다시 읽히면 따라간다).

import type { ConfigView } from "@pitwall/rpc-client";
import type { Action } from "@pitwall/keymap";
import { type ReactNode, useEffect, useMemo, useRef } from "react";

import type { EditorWorkspace } from "../editor/editor-workspace";
import { targetView } from "../editor/editor-ops";
import { editorContext } from "../editor/extensions/context";
import { useRegisterActions } from "../keymap/keymap-provider";
import { useProjectStrings } from "../layout/strings";
import { useRetirableNotify } from "../notifications/notifications";
import { useRpcQuery } from "../rpc/host";
import { showCodeActions } from "./code-actions";
import { useEditPreview } from "./edit-preview";
import type { LspEditStrings } from "./edit-strings";
import { createEditWindow, type EditWindow } from "./edit-window";
import type { LspServices } from "./lsp-sessions";
import { renameAt } from "./rename";
import type { EditFs } from "./workspace-edit";

/** 설정 스냅숏의 `[lsp] lightbulb`(없거나 틀리면 켬). */
export function lightbulbOf(config: ConfigView | undefined): boolean {
  const settings = config?.snapshot.files.find((f) => f.id === "settings")?.value as
    | { lsp?: { lightbulb?: unknown } }
    | undefined;
  return settings?.lsp?.lightbulb !== false;
}

/** 창의 Host 연결로 부르는 파일 RPC(접근 범위 = `worktree()`). */
export function editFs(
  services: LspServices,
  projectId: string,
  worktree: () => string | undefined,
): EditFs {
  const scope = () => ({ projectId, worktree: worktree() ?? null });
  return {
    stat: (path) => services.call("fs.stat", { ...scope(), path }),
    read: (path) => services.call("fs.read", { ...scope(), path }),
    write: (path, content, bom, expectedVersion) =>
      services.call("fs.write", { ...scope(), path, content, bom, expectedVersion }),
    create: (path) => services.call("fs.create", { ...scope(), path, kind: "file" }),
    rename: (from, to) => services.call("fs.rename", { ...scope(), from, to }),
    trash: (path) => services.call("fs.trash", { ...scope(), paths: [path] }),
  };
}

/** rename · 코드 액션 · 되돌리기 액션(키맵 표 시험도 이것을 부른다). */
export function lspEditActions(deps: {
  workspace: EditorWorkspace;
  strings: LspEditStrings["actions"];
  window(): EditWindow | undefined;
}): Action[] {
  const T = deps.strings;
  const view = () => targetView(deps.workspace);
  const lspView = () => {
    const v = view();
    return v && v.state.facet(editorContext)?.lsp ? v : undefined;
  };
  return [
    {
      id: "lsp.rename",
      label: T.rename,
      keywords: ["Rename", "Rename Symbol", "Refactor"],
      enabled: () => lspView() !== undefined,
      run: () => {
        const v = lspView();
        const win = deps.window();
        if (v && win) void renameAt(v, win);
      },
    },
    {
      id: "lsp.codeActions",
      label: T.codeActions,
      keywords: ["Show Context Actions", "Quick Fix", "Code Action", "Intention"],
      enabled: () => lspView() !== undefined,
      run: () => {
        const v = lspView();
        const win = deps.window();
        if (v && win) void showCodeActions(v, win);
      },
    },
    {
      id: "lsp.undoLastEdit",
      label: T.undoLast,
      keywords: ["Undo Rename", "Undo Refactoring", "Undo Batch Edit"],
      enabled: () => deps.window()?.applier.last !== undefined,
      run: () => void deps.window()?.undoLast(),
    },
  ];
}

export function useLspEditUi(workspace: EditorWorkspace): ReactNode {
  const strings = useProjectStrings().editor.lspEdit;
  const notify = useRetirableNotify();
  const config = useRpcQuery("config.get", {});
  const preview = useEditPreview(() => latest.current.strings);
  const latest = useRef({ strings, notify, lightbulb: lightbulbOf(config.data), ask: preview.ask });
  latest.current = { strings, notify, lightbulb: lightbulbOf(config.data), ask: preview.ask };
  const win = useRef<EditWindow>(undefined);
  const sessions = workspace.lsp;
  useEffect(() => {
    if (!sessions) return;
    const created = createEditWindow({
      services: sessions.services,
      fs: editFs(sessions.services, sessions.projectId, () => workspace.activeContext()?.worktree),
      workspace: {
        documents: workspace.documents,
        moved: (from, to) => workspace.moved(from, to),
        deleted: (path) => workspace.deleted(path),
      },
      preview: (files, label) => latest.current.ask(files, label),
      strings: () => latest.current.strings,
      notify: (input) => latest.current.notify(input),
      lightbulb: () => latest.current.lightbulb,
    });
    win.current = created;
    return () => {
      created.dispose();
      if (win.current === created) win.current = undefined;
    };
  }, [sessions, workspace]);
  const actions = useMemo(
    () => lspEditActions({ workspace, strings: strings.actions, window: () => win.current }),
    [workspace, strings.actions],
  );
  useRegisterActions(actions);
  return preview.element;
}
