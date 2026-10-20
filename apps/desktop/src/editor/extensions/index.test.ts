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
};

it("assembles one extension per feature, in list order", () => {
  expect(EDITOR_FEATURES.map((f) => f.id)).toEqual([
    "context",
    "base",
    "theme",
    "readOnly",
    "cursorReport",
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

it("makes a read-only viewer that still shows a cursor", () => {
  const view = new EditorView({
    parent: document.body,
    state: EditorState.create({ doc: "abc", extensions: editorExtensions(context) }),
  });
  expect(view.state.readOnly).toBe(true);
  expect(view.contentDOM.getAttribute("contenteditable")).toBe("true");
  expect(view.dom.querySelector(".cm-lineNumbers")).not.toBeNull();
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
  view.dispatch({ selection: { anchor: 6 } });
  expect(first).toHaveBeenLastCalledWith({ line: 2, column: 3 });

  const second = vi.fn();
  view.dispatch(reconfigureEditor({ ...context, onCursor: second }));
  view.dispatch({ selection: { anchor: 1 } });
  expect(second).toHaveBeenLastCalledWith({ line: 1, column: 2 });
  expect(first).toHaveBeenCalledTimes(1);
  view.destroy();
});
