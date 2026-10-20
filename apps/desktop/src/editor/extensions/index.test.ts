// @vitest-environment happy-dom
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { Judgment } from "@pitwall/rpc-client";
import { expect, it } from "vitest";

import { EDITOR_FEATURES, type EditorFeature, editorExtensions } from ".";

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
const context = { path: "/p/a.rs", judgment, readOnly: false };

it("assembles one extension per feature, in list order", () => {
  expect(EDITOR_FEATURES.map((f) => f.id)).toEqual(["base", "theme", "readOnly", "navigation"]);
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
