// 뷰어 종류 표(12-m1-plan M1-09): 종류(viewer-kinds.ts) → 그 종류를 그리는 컴포넌트. 에디터 탭(editor-panel.tsx)은 문서를 읽은 뒤
// 이 표에서 뷰어를 골라 그린다. 읽기 상태(읽는 중 · 실패 · 너무 큼 · UTF-8 아님)는 탭이 그리고, 뷰어는 읽힌 문서만 받는다.
//
// 뷰어를 더하거나 바꾸는 법: 컴포넌트를 `ViewerProps` 로 만들고 아래 표의 그 종류 한 줄을 바꾼다. 내용을 어디서 받는지는
// viewer-kinds.ts 의 `VIEWER_READS` 다 — `text` 뷰어는 `doc.text`(여러 탭이 나누는 한 벌)를, `self` 뷰어는 `doc.stat` 의 경로를
// 받아 필요한 범위를 `fs.read` 로 직접 읽는다.
//
//   image: ImageViewer,     // M1-22
//   hex: HexViewer,         // M1-23

import type { IDockviewPanelProps } from "dockview-react";
import type { ComponentType } from "react";

import { useProjectStrings } from "../layout/strings";
import type { DocumentState } from "./documents";
import { EditorNotice } from "./notice";
import { TextViewer } from "./text-viewer";
import type { ViewerKind } from "./viewer-kinds";

export interface ViewerProps {
  /** 탭(dockview 패널)의 api. 패널 id · 보임 여부를 쓴다. */
  panel: IDockviewPanelProps["api"];
  /** 정규화한 실제 경로. */
  path: string;
  doc: Extract<DocumentState, { status: "text" | "external" }>;
}

/** 아직 뷰어가 없는 종류. 그 사실과 외부 도구로 여는 버튼을 보인다. */
function PendingViewer({ doc }: ViewerProps) {
  const T = useProjectStrings().editor;
  return <EditorNotice text={T.noViewer(doc.kind)} />;
}

export const VIEWERS: Readonly<Record<ViewerKind, ComponentType<ViewerProps>>> = {
  text: TextViewer,
  // Markdown 미리보기(M1-22)가 생기기 전에는 텍스트로 보인다.
  markdown: TextViewer,
  image: PendingViewer,
  hex: PendingViewer,
};
