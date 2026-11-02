// 포맷(15-m4-plan M4-13)의 부품: 문서의 들여쓰기 판정(서버에 보내는 `tabSize` · `insertSpaces`), 서버의 큰 편집을 바뀐 곳만으로
// 나누기(커서가 같은 낱말에 남게), Host 오류 코드가 Rust 와 같은지.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { EditorState, Text } from "@codemirror/state";
import type { LspTextEdit } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import { detectIndent, FORMAT_ERROR, formatOptions, refineEdits } from "./format";
import { applyTextChanges, EditShapeError, textChanges } from "./workspace-edit";

const doc = (text: string) => Text.of(text.split("\n"));
const edit = (l1: number, c1: number, l2: number, c2: number, newText: string): LspTextEdit => ({
  range: { start: { line: l1, character: c1 }, end: { line: l2, character: c2 } },
  newText,
});

describe("indentation sent to the formatter", () => {
  it("reads the most common step between indented lines", () => {
    expect(detectIndent(doc("fn a() {\n    if x {\n        y\n    }\n}\n"), 8)).toEqual({
      tabSize: 4,
      insertSpaces: true,
    });
    expect(detectIndent(doc("a:\n  b:\n    c: 1\n  d: 2\n"), 4)).toEqual({
      tabSize: 2,
      insertSpaces: true,
    });
    // 한 줄만 깊게 들여써도 잦은 값이 이긴다.
    expect(detectIndent(doc("a\n  b\n  c\n    d\n  e\n          f\n"), 4)).toEqual({
      tabSize: 2,
      insertSpaces: true,
    });
  });

  it("uses tabs when more lines start with a tab, keeping the editor's tab width", () => {
    expect(detectIndent(doc("fn a() {\n\tb();\n\tc();\n}\n"), 4)).toEqual({
      tabSize: 4,
      insertSpaces: false,
    });
  });

  it("has nothing to say without indented lines, and the editor's tab width fills in", () => {
    expect(detectIndent(doc(""), 4)).toBeUndefined();
    expect(detectIndent(doc("a\nb\n\n"), 4)).toBeUndefined();
    expect(formatOptions(EditorState.create({ doc: "a\nb\n" }))).toEqual({
      tabSize: 4,
      insertSpaces: true,
    });
    expect(
      formatOptions(
        EditorState.create({ doc: "a\n   b\n", extensions: EditorState.tabSize.of(2) }),
      ),
    ).toEqual({ tabSize: 3, insertSpaces: true });
  });
});

describe("splitting a formatter's edits", () => {
  it("turns a whole-document replacement into the changed spots only, with the same result", () => {
    const text = "fn  main( ){\nlet x=1;\n}\n";
    const formatted = "fn main() {\n    let x = 1;\n}\n";
    const whole = [edit(0, 0, 3, 0, formatted)];
    const refined = refineEdits(text, whole);
    expect(refined.length).toBeGreaterThan(1);
    // 바뀌지 않은 낱말(`main`, `let`)은 어느 편집의 범위에도 들지 않는다.
    const covered = (offset: number) =>
      textChanges(text, refined).some((c) => c.from < offset && offset < c.to);
    expect(covered(text.indexOf("main") + 2)).toBe(false);
    expect(covered(text.indexOf("let") + 1)).toBe(false);
    expect(applyTextChanges(text, textChanges(text, refined))).toBe(formatted);
  });

  it("drops edits that change nothing and keeps the server's order of inserts at one spot", () => {
    const text = "a\nb\n";
    expect(refineEdits(text, [edit(0, 0, 0, 1, "a")])).toEqual([]);
    const refined = refineEdits(text, [edit(1, 0, 1, 0, "x"), edit(1, 0, 1, 0, "y")]);
    expect(applyTextChanges(text, textChanges(text, refined))).toBe("a\nxyb\n");
  });

  it("refuses overlapping edits", () => {
    expect(() => refineEdits("abcdef", [edit(0, 0, 0, 3, "x"), edit(0, 2, 0, 4, "y")])).toThrow(
      EditShapeError,
    );
  });
});

describe("error codes", () => {
  it("keeps the format refusal code of the Host (crates/pw-lsp/src/format.rs)", () => {
    const rust = readFileSync(
      resolve(__dirname, "../../../../crates/pw-lsp/src/format.rs"),
      "utf8",
    );
    expect(rust).toContain(`pub const FORMAT_REFUSED: i32 = ${FORMAT_ERROR.REFUSED};`);
  });
});
