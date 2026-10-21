import { SearchQuery } from "@codemirror/search";
import { EditorSelection, EditorState, type StateCommand } from "@codemirror/state";
import { describe, expect, it } from "vitest";

import {
  countMatches,
  duplicateLine,
  MATCH_COUNT_CAP,
  MatchScan,
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
      counting: false,
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
    expect(countMatches(state, new SearchQuery({ search: "(", regexp: true }))).toMatchObject({
      total: 0,
      capped: false,
      counting: false,
    });
  });

  it("stops at the cap", () => {
    const state = EditorState.create({ doc: "a".repeat(MATCH_COUNT_CAP + 5) });
    expect(countMatches(state, new SearchQuery({ search: "a" }))).toMatchObject({
      total: MATCH_COUNT_CAP,
      capped: true,
    });
  });

  // 조각으로 나눠 센 것(조각 1글자 = 줄마다 따로)이 문서 전체를 한 커서로 센 것과 같다 — 빈 일치 · 줄 처음 · 줄 끝 · 단어 ·
  // 대소문자 · 줄을 넘는 쿼리(나누지 않음)를 섞어.
  it("counts the same matches in line-aligned chunks as in one pass over the document", () => {
    const doc = "abc\ndef\n\nxyz Abc\nfn  bee(x) abcabc\n\u0130i ı\n";
    const queries = [
      new SearchQuery({ search: "x*", regexp: true }),
      new SearchQuery({ search: "^", regexp: true }),
      new SearchQuery({ search: "$", regexp: true }),
      new SearchQuery({ search: String.raw`\b\w`, regexp: true }),
      new SearchQuery({ search: "abc" }),
      new SearchQuery({ search: "abc", caseSensitive: true }),
      new SearchQuery({ search: "abc", wholeWord: true }),
      new SearchQuery({ search: "i" }),
      new SearchQuery({ search: String.raw`c\nd` }),
      new SearchQuery({ search: String.raw`c\nd`, regexp: true }),
      new SearchQuery({ search: String.raw`\s+`, regexp: true }),
    ];
    const state = EditorState.create({ doc });
    for (const query of queries) {
      const expected: { from: number; to: number }[] = [];
      const cursor = query.getCursor(state);
      for (let next = cursor.next(); !next.done; next = cursor.next()) expected.push(next.value);
      const scan = new MatchScan(state, query, MATCH_COUNT_CAP, 1);
      while (!scan.step(0));
      expect(scan.count({ from: -1, to: -1 }).total, query.search).toBe(expected.length);
      expected.forEach((m, i) => expect(scan.count(m).current, query.search).toBe(i + 1));
    }
  });

  it("stops after the time budget and goes on where it stopped, finding the current match as it goes", () => {
    const line = "no match here\n";
    const doc = line.repeat(2000) + "needle\n" + line.repeat(2000) + "needle\n";
    const state = EditorState.create({ doc });
    const scan = new MatchScan(state, new SearchQuery({ search: "needle" }), MATCH_COUNT_CAP, 1024);
    const second = { from: doc.lastIndexOf("needle"), to: doc.lastIndexOf("needle") + 6 };
    expect(scan.step(0)).toBe(false);
    expect(scan.count(second)).toEqual({
      total: 0,
      capped: false,
      counting: true,
      current: undefined,
    });
    let steps = 1;
    while (!scan.step(0)) steps++;
    expect(steps).toBeGreaterThan(50);
    expect(scan.count(second)).toEqual({ total: 2, capped: false, counting: false, current: 2 });
  });
});
