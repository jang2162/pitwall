// LSP 스니펫 → CodeMirror 템플릿(15-m4-plan M4-11): 탭 정지 · 자리 표시 · 고르기는 필드로, 변환 · 정규식은 글자로, 글의 중괄호는
// CodeMirror 가 필드로 읽지 않게 감싼다.

import { snippet } from "@codemirror/autocomplete";
import { EditorState, type Transaction } from "@codemirror/state";
import { describe, expect, it } from "vitest";

import { snippetTemplate, snippetText } from "./snippet";

/** 템플릿을 빈 문서에 CodeMirror 스니펫으로 넣은 글과 처음 고른 글. */
function inserted(lsp: string) {
  const state = EditorState.create({ doc: "" });
  let tr: Transaction | undefined;
  snippet(snippetTemplate(lsp))({ state, dispatch: (t) => void (tr = t) }, null, 0, 0);
  const main = tr!.state.selection.main;
  return { doc: tr!.state.doc.toString(), selected: tr!.state.sliceDoc(main.from, main.to) };
}

describe("snippetTemplate", () => {
  it("turns tab stops, placeholders and the final stop into CodeMirror fields", () => {
    expect(snippetTemplate("scale(${1:width}, ${2:height})$0")).toBe(
      "scale(${1:width}, ${2:height})${0}",
    );
    expect(snippetTemplate("fn $1() {\n\t$0\n}")).toBe("fn ${1}() \\{\n\t${0}\n\\}");
    expect(snippetTemplate("${1}")).toBe("${1}");
  });

  it("keeps the first choice, the default of a variable and nested placeholders' text", () => {
    expect(snippetTemplate("${1|one,two|}")).toBe("${1:one}");
    expect(snippetTemplate("${TM_SELECTED_TEXT:sel} $TM_FILENAME")).toBe("sel TM_FILENAME");
    expect(snippetTemplate("${1:outer ${2:inner}}")).toBe("${1:outer inner}");
  });

  it("puts transforms and escapes in as text", () => {
    expect(snippetTemplate("${1/(.*)/${1:/upcase}/}x")).toBe("$\\{1/(.*)/$\\{1:/upcase\\}/\\}x");
    expect(snippetTemplate("cost \\$5 \\} and $")).toBe("cost $5 \\} and $");
    // 템플릿을 CodeMirror 의 필드로 읽지 않는 중괄호 · `#{`.
    expect(snippetTemplate("#{x} {y}")).toBe("#\\{x\\} \\{y\\}");
  });

  it("drops braces CodeMirror cannot hold inside a placeholder", () => {
    expect(snippetTemplate("${1:a{b\\}c}")).toBe("${1:abc}");
  });

  it("keeps only the first line of a multi-line placeholder as the field", () => {
    expect(snippetTemplate("f(${1:a\nb})$0")).toBe("f(${1:a}\nb)${0}");
    expect(snippetTemplate("${1:\n\tx}")).toBe("${1}\n\tx");
    // CodeMirror 가 넣는 글: 원문(`${1:a`)이 아니라 기본 글이고 첫 필드가 골라져 있다.
    expect(inserted("f(${1:a\nb})$0")).toEqual({ doc: "f(a\nb)", selected: "a" });
    expect(inserted("match x {\n\t${1:pat} => ${2:()},\n}")).toEqual({
      doc: "match x {\n  pat => (),\n}",
      selected: "pat",
    });
  });

  it("reads the plain text of a snippet", () => {
    expect(snippetText("scale(${1:width}, $2)$0")).toBe("scale(width, )");
  });
});
