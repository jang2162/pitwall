// 자동완성 · 시그니처 도움말의 키맵 액션(15-m4-plan M4-11, 10-ui §4.2 「코드 (LSP · 편집)」). 기능은 ./completion.ts · ./signature.ts
// 에 있고 여기는 창의 키맵에 거는 자리다. 기본 키는 extensions/keymap-default/keymaps/default.toml 의 「코드 (LSP · 편집)」 묶음이고
// 글자 영역(`editorFocus`)에서만 돈다 — 자동완성 `⌃Space` · `⌥Space`(macOS 의 두 번째 키 — 10-ui §4.2), 시그니처 `⌘P`.
//
// 에디터가 있으면 늘 켜져 있다 — 문서가 언어 서버에 열리지 않았으면(미신뢰 · 서버 없음 · 플레인 텍스트) 아무것도 하지 않는다. 그래서
// macOS 의 `⌥Space` 가 그 문서에 줄 바꿈 없는 공백을 넣지 않는다. 키맵 엔진은 한글 조합 중인 키를 보지 않는다.

import type { Action } from "@pitwall/keymap";

import { targetView } from "../editor/editor-ops";
import type { EditorWorkspace } from "../editor/editor-workspace";
import type { ProjectStrings } from "../layout/strings";
import { triggerCompletion } from "./completion";
import { triggerSignatureHelp } from "./signature";

export function lspCompletionActions(deps: {
  workspace: EditorWorkspace;
  strings: ProjectStrings["editor"]["actions"];
}): Action[] {
  const T = deps.strings;
  const hasView = () => targetView(deps.workspace) !== undefined;
  return [
    {
      id: "editor.completion",
      label: T.completion,
      keywords: ["Basic Completion", "Code Completion", "Autocomplete", "Suggest"],
      enabled: hasView,
      run: () => {
        const view = targetView(deps.workspace);
        if (view) triggerCompletion(view);
      },
    },
    {
      id: "editor.parameterInfo",
      label: T.parameterInfo,
      keywords: ["Parameter Info", "Signature Help"],
      enabled: hasView,
      run: () => {
        const view = targetView(deps.workspace);
        if (view) triggerSignatureHelp(view);
      },
    },
  ];
}
