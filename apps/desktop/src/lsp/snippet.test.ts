// LSP 스니펫 → CodeMirror 템플릿(15-m4-plan M4-11): 탭 정지 · 자리 표시 · 고르기는 필드로, 변환 · 정규식은 글자로, 글의 중괄호는
// CodeMirror 가 필드로 읽지 않게 감싼다.

import { describe, expect, it } from "vitest";

import { snippetTemplate, snippetText } from "./snippet";

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

  it("reads the plain text of a snippet", () => {
    expect(snippetText("scale(${1:width}, $2)$0")).toBe("scale(width, )");
  });
});
