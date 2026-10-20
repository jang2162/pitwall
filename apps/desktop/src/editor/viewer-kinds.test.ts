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

it("lets a tab override the viewer unless the override needs text the document does not have", () => {
  const textDoc = { status: "text", kind: "text" } as const;
  const binaryDoc = { status: "external", kind: "hex" } as const;
  expect(chooseViewer(textDoc, undefined)).toBe("text");
  expect(chooseViewer(textDoc, "hex")).toBe("hex");
  expect(chooseViewer(binaryDoc, "text")).toBe("hex");
  expect(chooseViewer(binaryDoc, "image")).toBe("image");
});
