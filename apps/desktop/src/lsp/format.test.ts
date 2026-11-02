// 포맷(15-m4-plan M4-13)의 부품: 문서의 들여쓰기 판정(서버에 보내는 `tabSize` · `insertSpaces` — VS Code 규칙 + 주석 · 문자열
// 줄 건너뛰기), 서버의 큰 편집을 바뀐 곳만으로 나누기(커서가 같은 낱말에 남게 — 줄 단위 먼저, CRLF 글), Host 오류 코드가 Rust 와
// 같은지.

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
    // 4 를 더 많이 셌어도 2 를 그 절반 이상 셌으면 2(VS Code 규칙).
    expect(detectIndent(doc("x\n    a\n      b\n          c\n"), 4)).toEqual({
      tabSize: 2,
      insertSpaces: true,
    });
  });

  it("keeps a two-space interface with multi-line JSDoc at two, and Javadoc at four", () => {
    // M4-13 리뷰 H1: ` * ` 줄이 차이 1 을 두 번씩 만들어 1 을 골랐다.
    const jsdoc = [
      "export interface Options {",
      "  /**",
      "   * The name.",
      "   */",
      "  name: string;",
      "  /**",
      "   * The count.",
      "   */",
      "  count: number;",
      "  /**",
      "   * The label.",
      "   */",
      "  label: string;",
      "}",
      "",
    ].join("\n");
    expect(detectIndent(doc(jsdoc), 4)).toEqual({ tabSize: 2, insertSpaces: true });
    const javadoc = [
      "public class A {",
      "    /**",
      "     * Makes one.",
      "     *",
      "     * @param x the value",
      "     */",
      "    public A(int x) {",
      "        this.x = x;",
      "    }",
      "}",
      "",
    ].join("\n");
    expect(detectIndent(doc(javadoc), 2)).toEqual({ tabSize: 4, insertSpaces: true });
  });

  it("does not read indentation inside block comments or multi-line strings", () => {
    const docstring = [
      "def f():",
      '    """',
      "     odd",
      "       text",
      '    """',
      "    return 1",
      "",
    ].join("\n");
    expect(detectIndent(doc(docstring), 2)).toEqual({ tabSize: 4, insertSpaces: true });
    const comment = ["fn a() {", "    /*", "       note", "     */", "    b();", "}", ""].join(
      "\n",
    );
    expect(detectIndent(doc(comment), 2)).toEqual({ tabSize: 4, insertSpaces: true });
    const template = ["function a() {", "    return `", "  x", "   y`;", "}", ""].join("\n");
    expect(detectIndent(doc(template), 2)).toEqual({ tabSize: 4, insertSpaces: true });
    // 글롭의 `/*` 는 주석을 열지 않는다 — 뒤의 줄이 근거로 남는다.
    expect(detectIndent(doc("paths: src/*.ts\nx:\n  y: 1\n  z:\n    w: 2\n"), 4)).toEqual({
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

  it("never sends one, and falls back to the editor's tab width without evidence", () => {
    // 한 칸씩만 들여쓴 줄 · 들여쓰기가 없는 문서 → 에디터의 탭 너비.
    expect(detectIndent(doc("a\n b\n  c\n   d\n"), 4)).toEqual({ tabSize: 4, insertSpaces: true });
    expect(detectIndent(doc(""), 4)).toEqual({ tabSize: 4, insertSpaces: true });
    expect(formatOptions(EditorState.create({ doc: "a\nb\n" }))).toEqual({
      tabSize: 4,
      insertSpaces: true,
    });
    // 쉼표 뒤 낱말 자리에 맞춘 줄은 정렬이라 세지 않는다.
    expect(
      formatOptions(
        EditorState.create({
          doc: "const a = 1,\n      b = 2;\n",
          extensions: EditorState.tabSize.of(2),
        }),
      ),
    ).toEqual({ tabSize: 2, insertSpaces: true });
    // 근거가 그 한 줄뿐이어도 3 은 고를 수 있는 너비다(VS Code 규칙 — 1 만 없다).
    expect(
      formatOptions(
        EditorState.create({ doc: "a\n   b\n", extensions: EditorState.tabSize.of(2) }),
      ),
    ).toEqual({ tabSize: 3, insertSpaces: true });
  });

  it("reads the two-space live fixture as two (crates/pw-lsp/tests/fixtures — live TS keeps it)", () => {
    const fixture = readFileSync(
      resolve(
        __dirname,
        "../../../../crates/pw-lsp/tests/fixtures/typescript/src/m4_13_two_space.ts",
      ),
      "utf8",
    );
    expect(detectIndent(doc(fixture), 4)).toEqual({ tabSize: 2, insertSpaces: true });
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

  it("splits CRLF text from the server at line breaks, not between CR and LF", () => {
    // M4-13 리뷰 M1: `\r` 만 넣는 조각이 생겨 4 줄이 7 줄이 되었다.
    const text = "fn main(){\nlet x=1;\n}\n";
    const refined = refineEdits(text, [edit(0, 0, 3, 0, "fn main() {\r\n    let x = 1;\r\n}\r\n")]);
    expect(refined.some((e) => e.newText.includes("\r"))).toBe(false);
    expect(applyTextChanges(text, textChanges(text, refined))).toBe(
      "fn main() {\n    let x = 1;\n}\n",
    );
    // 줄 끝만 바꾸는 편집은 문서에 바뀔 것이 없다.
    expect(refineEdits("a\nb\n", [edit(0, 0, 2, 0, "a\r\nb\r\n")])).toEqual([]);
  });

  it("re-indents a large document line by line, leaving unchanged words outside every edit", () => {
    // M4-13 리뷰 L1: 글자 diff 하나로는 800 줄부터 덩어리 하나로 물러났다.
    const body = Array.from({ length: 4000 }, (_, i) => `  value${i} = compute(${i});`);
    const text = ["fn a() {", ...body, "}", ""].join("\n");
    const formatted = ["fn a() {", ...body.map((l) => `  ${l}`), "}", ""].join("\n");
    const refined = refineEdits(text, [edit(0, 0, 4002, 0, formatted)]);
    expect(refined.length).toBe(4000);
    const changes = textChanges(text, refined);
    expect(applyTextChanges(text, changes)).toBe(formatted);
    const word = text.indexOf("value3999") + 3;
    expect(changes.some((c) => c.from < word && word < c.to)).toBe(false);
  });

  it("does not split a surrogate pair", () => {
    const text = "a😀b\n";
    const refined = refineEdits(text, [edit(0, 0, 1, 0, "a😁b\n")]);
    for (const e of refined)
      for (const p of [e.range.start, e.range.end]) {
        const code = text.charCodeAt(p.character);
        expect(code >= 0xdc00 && code <= 0xdfff, JSON.stringify(e)).toBe(false);
      }
    expect(applyTextChanges(text, textChanges(text, refined))).toBe("a😁b\n");
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
