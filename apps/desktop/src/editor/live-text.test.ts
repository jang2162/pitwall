// @vitest-environment happy-dom
// 살아 있는 텍스트(live-text.ts, 12-m1-plan M1-13): 문서가 텍스트 · undo 기록 · 미저장 여부를 갖고, 뷰를 다시 만들거나 다른
// 뷰에서 고쳐도 그것이 남는다. 줄바꿈 · BOM 은 바이트 그대로 돌아간다.

import { EditorSelection } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it } from "vitest";

import { LiveText, type LiveTextInit } from "./live-text";

const views: EditorView[] = [];

afterEach(() => {
  for (const view of views.splice(0)) view.destroy();
});

/** 탭 `panelId` 의 뷰를 만들어 문서에 붙인다(텍스트 뷰어가 하는 것과 같다). 돌려준 `close` 가 맡기고 없앤다. */
function open(live: LiveText, panelId: string) {
  const made = live.createState(panelId, []);
  const view = new EditorView({
    parent: document.body,
    state: made.state,
    dispatchTransactions: (trs, v) => live.dispatch(trs, v),
  });
  views.push(view);
  const detach = live.attach(panelId, view, () => made.scroll);
  return {
    view,
    close() {
      detach();
      view.destroy();
    },
  };
}

/** 사용자가 친 것처럼 `at` 에 넣는다. */
function type(view: EditorView, at: number, insert: string) {
  view.dispatch({
    changes: { from: at, insert },
    selection: { anchor: at + insert.length },
    userEvent: "input.type",
  });
}

const lf = (text: string): LiveTextInit => ({ text, lineEnding: "lf", bom: false });

describe("live text", () => {
  it("shares edits between the views of one document, and undoes them from either", () => {
    const live = new LiveText(lf("one\ntwo"));
    const a = open(live, "a");
    const b = open(live, "b");
    type(a.view, 3, "!");
    expect(b.view.state.doc.toString()).toBe("one!\ntwo");
    expect(live.text.toString()).toBe("one!\ntwo");
    expect(live.dirty).toBe(true);

    // b 의 undo 가 a 에서 고친 것을 되돌린다 — 기록은 문서 하나다.
    expect(live.undo(b.view)).toBe(true);
    expect(a.view.state.doc.toString()).toBe("one\ntwo");
    expect(live.dirty).toBe(false);
    expect(live.redo(a.view)).toBe(true);
    expect(b.view.state.doc.toString()).toBe("one!\ntwo");
  });

  it("keeps undo history, the selection and unsaved text when the tab's view is made again", () => {
    const live = new LiveText(lf("alpha\nbeta\ngamma"));
    const first = open(live, "tab");
    type(first.view, 5, " one");
    first.view.dispatch({ selection: EditorSelection.range(0, 5) });
    first.close();

    // 뷰가 없는 동안 같은 문서의 다른 탭이 맨 앞에 줄을 넣는다 — 맡긴 선택이 함께 옮겨진다.
    const other = open(live, "other");
    type(other.view, 0, "zero\n");

    const again = open(live, "tab");
    expect(again.view.state.doc.toString()).toBe("zero\nalpha one\nbeta\ngamma");
    expect(again.view.state.selection.main).toMatchObject({ from: 5, to: 10 });
    expect(live.dirty).toBe(true);
    expect(live.undo(again.view)).toBe(true);
    expect(live.undo(again.view)).toBe(true);
    expect(again.view.state.doc.toString()).toBe("alpha\nbeta\ngamma");
    expect(live.dirty).toBe(false);
  });

  it("is clean again only when the text equals what is on disk", () => {
    const live = new LiveText(lf("a"));
    const { view } = open(live, "t");
    const flips: boolean[] = [];
    live.onDirtyChange(() => flips.push(live.dirty));
    type(view, 1, "b");
    type(view, 2, "c");
    const saved = live.text;
    live.markClean(saved);
    type(view, 3, "d");
    view.dispatch({ changes: { from: 3, to: 4 }, userEvent: "delete" });
    expect(flips).toEqual([true, false, true, false]);
  });

  it("serializes with the file's line ending, and leaves mixed endings and the BOM byte for byte", () => {
    const crlf = new LiveText({ text: "a\r\nb\r\n", lineEnding: "crlf", bom: true });
    type(open(crlf, "t").view, 1, "!");
    expect(crlf.serialize()).toBe("a!\r\nb\r\n");
    expect(crlf.bom).toBe(true);

    const cr = new LiveText({ text: "a\rb", lineEnding: "cr", bom: false });
    type(open(cr, "t").view, 3, "!");
    expect(cr.serialize()).toBe("a\rb!");

    // 섞였으면 `\n` 으로만 나눠 `\r` 을 글자로 둔다 — 고치지 않은 줄의 바이트가 그대로다.
    const mixed = new LiveText({ text: "a\r\nb\nc\r\n", lineEnding: "mixed", bom: false });
    const { view } = open(mixed, "t");
    expect(view.state.doc.lines).toBe(4);
    type(view, view.state.doc.line(2).to, "!");
    expect(mixed.serialize()).toBe("a\r\nb!\nc\r\n");
  });

  it("takes text from disk as a diff, keeping the cursor on its line and the edit undoable", () => {
    const lines = Array.from({ length: 50 }, (_, i) => `line ${i + 1}`);
    const live = new LiveText(lf(lines.join("\n")));
    const { view } = open(live, "t");
    const at = view.state.doc.line(30).from + 2;
    view.dispatch({ selection: { anchor: at } });

    const changed = [...lines];
    changed[4] = "line 5 (changed on disk)";
    changed[44] = "line 45 (changed on disk)";
    expect(live.replace(lf(changed.join("\n")))).toBe(true);
    expect(view.state.doc.toString()).toBe(changed.join("\n"));
    expect(view.state.doc.lineAt(view.state.selection.main.head).number).toBe(30);
    expect(view.state.selection.main.head - view.state.doc.line(30).from).toBe(2);
    expect(live.dirty).toBe(false);

    // 섞인 줄바꿈으로 바뀌면 줄 구분이 달라 넣지 않는다(문서를 새로 읽는다).
    expect(live.replace({ text: "a\r\nb\n", lineEnding: "mixed", bom: false })).toBe(false);
  });
});
