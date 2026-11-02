// 창에 포맷(15-m4-plan M4-13)을 붙인다: 키맵 액션 `lsp.format`(`⌘⌥L` · Win/Linux `Ctrl+Alt+L`, 에디터 포커스 — 10-ui §4.2 「코드」)이
// 활성 탭의 뷰를 포맷한다(format.ts). 문서 모델(`workspace.documents`)의 텍스트에만 넣고 디스크에 쓰지 않는다.

import type { Action } from "@pitwall/keymap";
import { useMemo, useRef } from "react";

import type { EditorWorkspace } from "../editor/editor-workspace";
import { targetView } from "../editor/editor-ops";
import { editorContext } from "../editor/extensions/context";
import { useRegisterActions } from "../keymap/keymap-provider";
import { useProjectStrings } from "../layout/strings";
import { useNotify } from "../notifications/notifications";
import { type FormatDeps, formatView } from "./format";
import type { LspFormatStrings } from "./format-strings";

/** 포맷 액션(키맵 표 시험도 이것을 부른다). */
export function lspFormatActions(deps: {
  workspace: EditorWorkspace;
  strings: LspFormatStrings;
  notify: FormatDeps["notify"];
}): Action[] {
  const { workspace } = deps;
  const lspView = () => {
    const v = targetView(workspace);
    return v && v.state.facet(editorContext)?.lsp ? v : undefined;
  };
  return [
    {
      id: "lsp.format",
      label: deps.strings.action,
      keywords: ["Reformat Code", "Format Document", "Format Selection", "Prettier"],
      enabled: () => lspView() !== undefined,
      run: () => {
        const v = lspView();
        if (!v) return;
        void formatView(v, {
          live: (path) => {
            const doc = workspace.documents.get(path);
            return doc?.status === "text" ? doc.live : undefined;
          },
          notify: deps.notify,
          strings: () => deps.strings,
        });
      },
    },
  ];
}

export function useLspFormatUi(workspace: EditorWorkspace): void {
  const strings = useProjectStrings().editor.lspFormat;
  const notify = useNotify();
  const latest = useRef(notify);
  latest.current = notify;
  const actions = useMemo(
    () =>
      lspFormatActions({
        workspace,
        strings,
        notify: (input) => latest.current(input),
      }),
    [workspace, strings],
  );
  useRegisterActions(actions);
}
