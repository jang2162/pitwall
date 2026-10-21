// @vitest-environment happy-dom
import { EditorState, Facet } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { Judgment } from "@pitwall/rpc-client";
import { expect, it, vi } from "vitest";

import {
  EDITOR_FEATURES,
  type EditorFeature,
  type EditorFeatureContext,
  editorContext,
  editorExtensions,
  reconfigureEditor,
} from ".";

const judgment: Judgment = {
  binary: false,
  encoding: "UTF-8",
  lossy: false,
  bom: null,
  lineEnding: "lf",
  longLines: false,
  sizeClass: "editable",
  editable: true,
  highlight: true,
};
const context: EditorFeatureContext = {
  panelId: "editor-1",
  path: "/p/a.rs",
  judgment,
  readOnly: false,
  language: "Rust",
  appearance: "light",
  onCursor: () => {},
  history: { undo: () => false, redo: () => false },
  font: {},
  createFindPanel: () => ({ dom: document.createElement("div"), top: true }),
};

it("assembles one extension per feature, in list order", () => {
  expect(EDITOR_FEATURES.map((f) => f.id)).toEqual([
    "context",
    "base",
    "theme",
    "highlight",
    "readOnly",
    "cursorReport",
    "editing",
    "operations",
    "navigation",
  ]);
  const seen: string[] = [];
  const spy = (id: string): EditorFeature => ({
    id,
    extension: (c) => {
      seen.push(`${id}:${c.path}`);
      return [];
    },
  });
  expect(editorExtensions(context, [spy("a"), spy("b")])).toHaveLength(2);
  expect(seen).toEqual(["a:/p/a.rs", "b:/p/a.rs"]);
});

it("edits a document it can write, and keeps a read-only one read-only with a cursor", () => {
  const make = (readOnly: boolean) =>
    new EditorView({
      parent: document.body,
      state: EditorState.create({
        doc: "abc",
        extensions: editorExtensions({ ...context, readOnly }),
      }),
    });
  const editable = make(false);
  expect(editable.state.readOnly).toBe(false);
  const locked = make(true);
  expect(locked.state.readOnly).toBe(true);
  expect(locked.contentDOM.getAttribute("contenteditable")).toBe("true");
  expect(locked.dom.querySelector(".cm-lineNumbers")).not.toBeNull();
  editable.destroy();
  locked.destroy();
});

// undo · redo 의 키는 창의 키맵 액션(`editor.undo` · `editor.redo`, ../editor-ops.ts)이다. 에디터도 잡으면 이벤트가 창까지 올라가
// 두 번 돈다(navigation.ts 머리 주석) — 그래서 에디터는 키를 잡지 않고 macOS 네이티브 메뉴의 `beforeinput` 만 받는다.
it("sends the native menu's undo and redo to the document's history and leaves the keys to the window", () => {
  const history = { undo: vi.fn(() => true), redo: vi.fn(() => true) };
  const view = new EditorView({
    parent: document.body,
    state: EditorState.create({
      doc: "abc",
      extensions: editorExtensions({ ...context, history }),
    }),
  });
  const key = (init: KeyboardEventInit) =>
    view.contentDOM.dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
    );
  key({ key: "z", code: "KeyZ", ctrlKey: true });
  key({ key: "Z", code: "KeyZ", keyCode: 90, ctrlKey: true, shiftKey: true });
  key({ key: "y", code: "KeyY", ctrlKey: true });
  expect(history.undo).not.toHaveBeenCalled();
  expect(history.redo).not.toHaveBeenCalled();
  const input = (inputType: string) =>
    view.contentDOM.dispatchEvent(
      new InputEvent("beforeinput", { inputType, bubbles: true, cancelable: true }),
    );
  input("historyUndo");
  input("historyRedo");
  expect(history.undo).toHaveBeenCalledTimes(1);
  expect(history.redo).toHaveBeenCalledTimes(1);
  view.destroy();
});

it("reconfigures each feature with a new context in the same view, keeping the selection", () => {
  // 언어를 보는 기능(M1-17 강조의 자리): 언어가 바뀌면 다시 불려야 한다.
  const languageFacet = Facet.define<string | null, string | null>({
    combine: (v) => v[0] ?? null,
  });
  const language: EditorFeature = {
    id: "language",
    extension: (c) => languageFacet.of(c.language),
  };
  const features = [...EDITOR_FEATURES, language];
  const view = new EditorView({
    parent: document.body,
    state: EditorState.create({ doc: "one\ntwo", extensions: editorExtensions(context, features) }),
  });
  view.dispatch({ selection: { anchor: 5 } });

  const next = { ...context, language: null, appearance: "dark" as const };
  view.dispatch(reconfigureEditor(next, features));
  expect(view.state.facet(languageFacet)).toBeNull();
  expect(view.state.facet(editorContext)).toBe(next);
  expect(view.state.selection.main.head).toBe(5);
  view.destroy();
});

it("reports the cursor through the context, and the reconfigured callback after a reconfigure", () => {
  const first = vi.fn();
  const view = new EditorView({
    parent: document.body,
    state: EditorState.create({
      doc: "one\ntwo",
      extensions: editorExtensions({ ...context, onCursor: first }),
    }),
  });
  // 뷰가 생길 때 한 번(상태바의 첫 값, operations.ts), 선택이 바뀌면 또.
  expect(first).toHaveBeenLastCalledWith({ line: 1, column: 1 });
  view.dispatch({ selection: { anchor: 6 } });
  expect(first).toHaveBeenLastCalledWith({ line: 2, column: 3 });

  const second = vi.fn();
  view.dispatch(reconfigureEditor({ ...context, onCursor: second }));
  view.dispatch({ selection: { anchor: 1 } });
  expect(second).toHaveBeenLastCalledWith({ line: 1, column: 2 });
  expect(first).toHaveBeenCalledTimes(2);
  view.destroy();
});
