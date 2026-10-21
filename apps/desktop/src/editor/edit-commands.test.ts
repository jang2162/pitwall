import { SearchQuery } from "@codemirror/search";
import { EditorSelection, EditorState, type StateCommand } from "@codemirror/state";
import { describe, expect, it } from "vitest";

import {
  countMatches,
  duplicateLine,
  MATCH_COUNT_CAP,
  moveLineDown,
  parseLineColumn,
  toggleLineComment,
} from "./edit-commands";

/** `|` 가 커서, `[` `]` 가 선택. */
function stateOf(text: string, readOnly = false): EditorState {
  const anchor = text.indexOf("[");
  if (anchor >= 0) {
    const head = text.indexOf("]") - 1;
    return EditorState.create({
      doc: text.replace("[", "").replace("]", ""),
      selection: EditorSelection.single(anchor, head),
      extensions: EditorState.readOnly.of(readOnly),
    });
  }
  const at = text.indexOf("|");
  return EditorState.create({
    doc: text.replace("|", ""),
    selection: EditorSelection.cursor(at),
    extensions: EditorState.readOnly.of(readOnly),
  });
}

/** 명령을 돌린 뒤의 텍스트(커서 · 선택 표시를 넣어서). 돌지 않았으면 `undefined`. */
function run(command: StateCommand, state: EditorState): string | undefined {
  let next: EditorState | undefined;
  const ran = command({ state, dispatch: (tr) => (next = tr.state) });
  if (!ran || !next) return undefined;
  const { from, to, empty } = next.selection.main;
  const doc = next.doc.toString();
  return empty
    ? doc.slice(0, from) + "|" + doc.slice(from)
    : doc.slice(0, from) + "[" + doc.slice(from, to) + "]" + doc.slice(to);
}

describe("duplicateLine", () => {
  it("copies the line below and moves the cursor onto the copy", () => {
    expect(run(duplicateLine, stateOf("a\nb|c\nd"))).toBe("a\nbc\nb|c\nd");
  });

  it("copies a selection right after it and selects the copy", () => {
    expect(run(duplicateLine, stateOf("x [ab] y"))).toBe("x ab[ab] y");
  });

  it("leaves a read-only document alone", () => {
    expect(run(duplicateLine, stateOf("a|b", true))).toBeUndefined();
  });
});

// 줄 삭제(`deleteLine`)는 뷰(커서를 세로로 옮김)가 있어야 해서 창 시험(editor-ops.test.tsx)이 본다.
it("moves lines with the CodeMirror command, but not in a read-only document", () => {
  expect(run(moveLineDown, stateOf("a|a\nb\nc"))).toBe("b\na|a\nc");
  expect(run(moveLineDown, stateOf("a|a\nb", true))).toBeUndefined();
});

describe("toggleLineComment", () => {
  const rust = toggleLineComment("//");
  const python = toggleLineComment("#");

  it("comments the cursor line with the language's token and moves down a line", () => {
    expect(run(rust, stateOf("fn a() {\n    le|t x = 1;\n}"))).toBe(
      "fn a() {\n    // let x = 1;\n}|",
    );
    expect(run(python, stateOf("x = 1|\ny = 2"))).toBe("# x = 1\ny = 2|");
  });

  it("uncomments when every non-blank selected line is a comment, keeping the indent", () => {
    expect(run(rust, stateOf("[    // a\n\n    //b]\nc"))).toBe("[    a\n\n    b]\nc");
  });

  it("comments selected lines at their shallowest indent and skips blank lines", () => {
    expect(run(python, stateOf("[  a\n\n    b\n]c"))).toBe("[  # a\n\n  #   b\n]c");
  });

  it("does nothing without a token or in a read-only document", () => {
    expect(run(toggleLineComment(""), stateOf("a|"))).toBeUndefined();
    expect(run(rust, stateOf("a|", true))).toBeUndefined();
  });
});

it("reads a line or line:column for go to line", () => {
  expect(parseLineColumn("120:5")).toEqual({ line: 120, column: 5 });
  expect(parseLineColumn(" 12 ")).toEqual({ line: 12, column: 1 });
  expect(parseLineColumn("7,3")).toEqual({ line: 7, column: 3 });
  for (const bad of ["", "abc", "0", "3:0", "1:2:3", "-1"])
    expect(parseLineColumn(bad), bad).toBeUndefined();
});

describe("countMatches", () => {
  const text = "fn a() {}\nfn  bee(x) {}\nlet fn_c = 1;\n// fn\n";

  it("counts regex matches and finds the selected one", () => {
    const query = new SearchQuery({ search: String.raw`fn\s+\w+`, regexp: true });
    expect(countMatches(EditorState.create({ doc: text }), query)).toEqual({
      total: 2,
      capped: false,
      current: undefined,
    });
    const second = text.indexOf("fn  bee");
    const state = EditorState.create({
      doc: text,
      selection: EditorSelection.single(second, second + "fn  bee".length),
    });
    expect(countMatches(state, query).current).toBe(2);
  });

  it("follows case and whole-word options, and gives 0 for a bad regex", () => {
    const state = EditorState.create({ doc: "Fn fn fn_x FN" });
    expect(countMatches(state, new SearchQuery({ search: "fn" })).total).toBe(4);
    expect(countMatches(state, new SearchQuery({ search: "fn", caseSensitive: true })).total).toBe(
      2,
    );
    expect(countMatches(state, new SearchQuery({ search: "fn", wholeWord: true })).total).toBe(3);
    expect(countMatches(state, new SearchQuery({ search: "(", regexp: true }))).toEqual({
      total: 0,
      capped: false,
    });
  });

  it("stops at the cap", () => {
    const state = EditorState.create({ doc: "a".repeat(MATCH_COUNT_CAP + 5) });
    expect(countMatches(state, new SearchQuery({ search: "a" }))).toMatchObject({
      total: MATCH_COUNT_CAP,
      capped: true,
    });
  });
});
