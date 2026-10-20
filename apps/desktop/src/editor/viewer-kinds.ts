// 뷰어 종류(12-m1-plan M1-09): 파일 하나를 어떤 뷰어로 보일지. 종류마다 그리는 컴포넌트는 viewers.tsx 의 `VIEWERS` 표에 있다.
// 이 파일은 컴포넌트 없이 종류만 정한다 — 문서 모델(documents.ts)이 읽기 전에 이것으로 무엇을 읽을지 고른다.
//
// 정하는 순서: 확장자(`BY_EXTENSION`) → 내용이 바이너리면 `hex` → `text`. 이미지(결정 M1-D11, svg 포함)는 확장자로만 정한다 —
// 내용을 텍스트로 읽지 않는다. 종류를 더하려면 `ViewerKind` · `VIEWER_READS` 에 한 줄씩, 필요하면 `BY_EXTENSION` 에 확장자를,
// viewers.tsx 의 `VIEWERS` 에 컴포넌트를 더한다.

import type { Judgment } from "@pitwall/rpc-client";

export type ViewerKind = "text" | "hex" | "image" | "markdown";

/**
 * 뷰어가 내용을 어디서 받는가. `text` 는 문서 모델이 파일 전체를 텍스트로 읽어 넘기고(여러 탭이 한 벌을 나눈다), `self` 는 뷰어가
 * 필요한 범위를 `fs.read` 로 직접 읽는다(이미지는 바이트 전체, hex 는 페이지 — M1-22 · M1-23).
 */
export const VIEWER_READS: Readonly<Record<ViewerKind, "text" | "self">> = {
  text: "text",
  markdown: "text",
  hex: "self",
  image: "self",
};

const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg"];

/** 소문자 확장자 → 종류. 여기 없으면 내용으로 정한다. */
const BY_EXTENSION: Readonly<Record<string, ViewerKind>> = {
  ...Object.fromEntries(IMAGE_EXTENSIONS.map((ext) => [ext, "image" as const])),
  md: "markdown",
  markdown: "markdown",
};

function extensionOf(path: string): string | undefined {
  const name = path.slice(Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")) + 1);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : undefined;
}

/** 이름만으로 정해지는 종류. 없으면 내용을 봐야 한다. */
export function viewerKindByName(path: string): ViewerKind | undefined {
  const ext = extensionOf(path);
  return ext === undefined ? undefined : BY_EXTENSION[ext];
}

/**
 * 탭이 그릴 뷰어. 탭의 덮어쓰기(`EditorTabParams.viewer`)가 있고 이 문서로 그릴 수 있으면 그것, 아니면 문서가 정한 종류. 텍스트를
 * 받는 뷰어(`VIEWER_READS` 가 `text`)는 텍스트로 읽힌 문서에만 쓸 수 있다 — 바이너리를 텍스트로 여는 읽기는 M1-23 이 문서 모델에
 * 더한다.
 */
export function chooseViewer(
  doc: { readonly status: "text" | "external"; readonly kind: ViewerKind },
  override: ViewerKind | undefined,
): ViewerKind {
  if (override === undefined) return doc.kind;
  if (VIEWER_READS[override] === "text" && doc.status !== "text") return doc.kind;
  return override;
}

/** 이름과 읽은 판정으로 정한 종류. */
export function viewerKindOf(path: string, judgment: Pick<Judgment, "binary">): ViewerKind {
  const byName = viewerKindByName(path);
  if (byName && VIEWER_READS[byName] === "self") return byName;
  if (judgment.binary) return "hex";
  return byName ?? "text";
}
