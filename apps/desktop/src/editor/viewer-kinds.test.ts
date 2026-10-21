import { expect, it } from "vitest";

import { chooseViewer, VIEWER_READS, viewerKindByName, viewerKindOf } from "./viewer-kinds";

const text = { binary: false };
const binary = { binary: true };

it("picks images by extension whatever the content, svg included", () => {
  for (const name of ["a.png", "b.JPG", "c.jpeg", "d.gif", "e.webp", "f.bmp", "g.ico"])
    expect(viewerKindOf(`/p/${name}`, binary)).toBe("image");
  // svg 는 텍스트지만 이미지로 보인다(스크립트가 돌지 않게 <img> 로만 그린다 — M1-22).
  expect(viewerKindOf("/p/h.svg", text)).toBe("image");
  expect(VIEWER_READS.image).toBe("self");
});

it("picks Markdown by extension, hex for binary content, and text otherwise", () => {
  expect(viewerKindOf("/p/README.md", text)).toBe("markdown");
  expect(viewerKindOf("/p/notes.markdown", text)).toBe("markdown");
  expect(viewerKindOf("/p/README.md", binary)).toBe("hex");
  expect(viewerKindOf("/p/a.out", binary)).toBe("hex");
  expect(viewerKindOf("/p/main.rs", text)).toBe("text");
  // 점으로 시작하는 이름은 확장자가 아니다.
  expect(viewerKindByName("/p/.png")).toBeUndefined();
  expect(viewerKindByName("C:\\p\\logo.PNG")).toBe("image");
});

it("lets a tab open any document as text or hex, and Markdown only from text", () => {
  const textDoc = { status: "text", kind: "text" } as const;
  const binaryDoc = { status: "external", kind: "hex" } as const;
  const imageDoc = { status: "external", kind: "image" } as const;
  expect(chooseViewer(textDoc, undefined)).toBe("text");
  expect(chooseViewer(textDoc, "hex")).toBe("hex");
  // 「텍스트로 열기」(M1-23): 텍스트로 읽지 않은 문서도 텍스트로 — 그 탭이 직접 읽는다.
  expect(chooseViewer(binaryDoc, "text")).toBe("text");
  expect(chooseViewer(imageDoc, "text")).toBe("text");
  expect(chooseViewer(imageDoc, "hex")).toBe("hex");
  // Markdown 미리보기는 텍스트로 읽은 문서만.
  expect(chooseViewer(binaryDoc, "markdown")).toBe("hex");
  expect(chooseViewer(textDoc, "markdown")).toBe("markdown");
  expect(chooseViewer(binaryDoc, "image")).toBe("image");
});
