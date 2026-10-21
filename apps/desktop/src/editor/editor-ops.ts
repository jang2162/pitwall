// 에디터 조작 액션(12-m1-plan M1-18, 10-ui §4.2 「검색 · 이동」 · 「코드」): 찾기 · 바꾸기 · 줄로 이동과 LSP 없이 되는 편집 키. 기본
// 키는 extensions/keymap-default/keymaps/default.toml 의 「에디터 조작」 묶음이고, 모두 `when` 으로 에디터 안에서만 돈다 — 편집 키는
// 글자 영역(`editorFocus`)에서만, 찾기 · 바꾸기는 찾기 막대 안(`editorFind`)에서도(extensions/operations.ts). 그래서 웰컴 창의 ⌘⌫
// (목록에서 제거) · 파일 트리 · 터미널의 같은 키와 겹치지 않는다. 키맵 엔진은 IME 조합 중인 키를 보지 않는다(packages/keymap
// engine.ts) — 한글을 조합하는 중의 ⌘D 등은 돌지 않는다.
//
// 대상은 포커스가 든 뷰다(같은 파일을 두 그룹이 보여도 포커스 쪽). 포커스가 에디터 밖이면(액션 찾기에서 고름) 활성 탭의 뷰.
// 줄 주석은 문서의 언어(language.ts — 사람이 고른 값이 있으면 그것)의 `line_comment` 로 한다. 그 언어의 줄 주석을 모르면(확장이
// 적지 않음 · 플레인 텍스트) 하지 않는다.
//
// undo · redo(`editor.undo` · `editor.redo`)도 여기 키맵 액션이다. Win/Linux 의 Edit 메뉴 「실행 취소」 · 「실행 복귀」 가 이 액션이라
// 키에 `when` 이 없다(기본 키맵 주석). 그래서 포커스가 에디터 밖의 입력 칸(찾기 막대 · 대화상자 · 터미널)에 있으면 꺼진다 — 키는
// 그 칸의 실행 취소로 가고, 메뉴로 골라도 보이지 않는 에디터를 되돌리지 않는다.

import { EditorView } from "@codemirror/view";
import type { Action } from "@pitwall/keymap";

import type { ProjectStrings } from "../layout/strings";
import {
  deleteLine,
  duplicateLine,
  moveLineDown,
  moveLineUp,
  toggleLineComment,
} from "./edit-commands";
import type { EditorWorkspace } from "./editor-workspace";
import { editorContext } from "./extensions";
import { historyCommand } from "./extensions/editing";
import { openFind } from "./find-bar";
import { getDocLanguage } from "./language";

/** 포커스가 든 에디터 뷰, 없으면 활성 탭의 뷰. */
export function targetView(workspace: EditorWorkspace): EditorView | undefined {
  const dom = document.activeElement?.closest(".cm-editor");
  const focused = dom instanceof HTMLElement ? EditorView.findFromDOM(dom) : null;
  if (focused) return focused;
  const panel = workspace.activeTab();
  return panel ? workspace.view(panel.id) : undefined;
}

/** 포커스가 에디터 글자 영역 밖의 입력 칸(input · textarea · contenteditable)에 있다. 찾기 막대는 `.cm-editor` 안에 있어 글자
 * 영역(`.cm-content`)으로 가른다. */
function typingOutsideEditor(): boolean {
  const el = document.activeElement;
  if (!(el instanceof HTMLElement) || el.closest(".cm-content")) return false;
  return (
    el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || el.isContentEditable
  );
}

export function editorOpsActions(deps: {
  workspace: EditorWorkspace;
  strings: ProjectStrings["editor"]["actions"];
  /** 줄로 이동 대화상자를 연다(goto-line.tsx). */
  openGotoLine(view: EditorView): void;
  /** 언어(Linguist 이름)의 줄 주석 기호(`lang.list`). 모르면 `undefined`. */
  lineComment(language: string): string | undefined;
}): Action[] {
  const T = deps.strings;
  const hasView = () => targetView(deps.workspace) !== undefined;
  const on = (run: (view: EditorView) => void) => () => {
    const view = targetView(deps.workspace);
    if (view) run(view);
  };
  const edit = (
    id: string,
    label: string,
    keywords: string[],
    command: (view: EditorView) => boolean,
  ): Action => ({ id, label, keywords, enabled: hasView, repeat: true, run: on(command) });
  const history = (
    id: string,
    label: string,
    keywords: string[],
    which: "undo" | "redo",
  ): Action => ({
    id,
    label,
    keywords,
    enabled: () => hasView() && !typingOutsideEditor(),
    repeat: true,
    run: on((view) => {
      const context = view.state.facet(editorContext);
      if (context) historyCommand(context, which)(view);
    }),
  });
  return [
    {
      id: "editor.find",
      label: T.find,
      keywords: ["Find", "Search"],
      enabled: hasView,
      run: on((view) => openFind(view, "find")),
    },
    {
      id: "editor.replace",
      label: T.replace,
      keywords: ["Replace"],
      enabled: hasView,
      run: on((view) => openFind(view, "replace")),
    },
    {
      id: "editor.gotoLine",
      label: T.gotoLine,
      keywords: ["Go to Line", "Line:Column"],
      enabled: hasView,
      run: on(deps.openGotoLine),
    },
    history("editor.undo", T.undo, ["Undo"], "undo"),
    history("editor.redo", T.redo, ["Redo"], "redo"),
    edit("editor.duplicateLine", T.duplicateLine, ["Duplicate Line or Selection"], duplicateLine),
    edit("editor.deleteLine", T.deleteLine, ["Delete Line"], deleteLine),
    edit("editor.moveLineUp", T.moveLineUp, ["Move Line Up"], moveLineUp),
    edit("editor.moveLineDown", T.moveLineDown, ["Move Line Down"], moveLineDown),
    edit("editor.commentLine", T.commentLine, ["Comment with Line Comment"], (view) => {
      const path = view.state.facet(editorContext)?.path;
      const token = path === undefined ? undefined : deps.lineComment(getDocLanguage(path));
      return token !== undefined && toggleLineComment(token)(view);
    }),
  ];
}
