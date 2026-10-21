// 뷰어 종류 표(12-m1-plan M1-09): 종류(viewer-kinds.ts) → 그 종류를 그리는 컴포넌트. 에디터 탭(editor-panel.tsx)은 문서를 읽은 뒤
// 이 표에서 뷰어를 골라 그린다. 탭이 뷰어를 덮어쓰면(`EditorTabParams.viewer`, M1-23 「텍스트로 열기」 · 「hex 로 열기」) 그
// 종류를 쓴다. 읽는 중 · 실패는 탭이 그리고, 뷰어는 읽힌 문서만 받는다.
//
// 뷰어를 더하거나 바꾸는 법: 컴포넌트를 `ViewerProps` 로 만들고 아래 표의 그 종류 한 줄을 바꾼다. 내용을 어디서 받는지는
// viewer-kinds.ts 의 `VIEWER_READS` 다 — `text` 뷰어는 `doc.live`(여러 탭이 나누는 편집 중인 한 벌, live-text.ts)를, `self` 뷰어는 `doc.stat` 의 경로를
// 받아 필요한 범위를 `fs.read` 로 직접 읽는다.
//
//   image: ImageViewer,     // M1-22
//   hex: HexViewer,         // M1-23
//
// 내용을 읽지 않은 문서(너무 큼 · UTF-8 아님)도 아래 `UNREAD_VIEWERS` 표가 그린다 — M1-23 이 [앞부분 보기] [hex] 화면 · 인코딩을
// 짐작한 읽기 전용 뷰어로 그 한 줄을 바꾼다.

import type { IDockviewPanelProps } from "dockview-react";
import type { ComponentType } from "react";

import { useProjectStrings } from "../layout/strings";
import type { DocumentState } from "./documents";
import type { EditorTabParams } from "./editor-tabs";
import { EditorNotice } from "./notice";
import { TextViewer } from "./text-viewer";
import type { ViewerKind } from "./viewer-kinds";

interface TabViewerProps<D> {
  /** 탭(dockview 패널)의 api. 패널 id · 보임 여부를 쓴다. */
  panel: IDockviewPanelProps["api"];
  /** 탭 params(정규화한 경로 · 워크트리 · 탭의 언어 덮어쓰기 등). */
  params: EditorTabParams;
  doc: D;
}

export type ViewerProps = TabViewerProps<Extract<DocumentState, { status: "text" | "external" }>>;

export type UnreadViewerProps = TabViewerProps<
  Extract<DocumentState, { status: "tooLarge" | "notUtf8" }>
>;

/** 아직 뷰어가 없는 종류. 그 사실과 외부 도구로 여는 버튼을 보인다. */
function PendingViewer({ doc, params }: ViewerProps) {
  const T = useProjectStrings().editor;
  return <EditorNotice text={T.noViewer(params.viewer ?? doc.kind)} />;
}

export const VIEWERS: Readonly<Record<ViewerKind, ComponentType<ViewerProps>>> = {
  text: TextViewer,
  // Markdown 미리보기(M1-22)가 생기기 전에는 텍스트로 보인다.
  markdown: TextViewer,
  image: PendingViewer,
  hex: PendingViewer,
};

const MIB = 1024 * 1024;

/** 바이트를 MiB 로(소수 한 자리). */
export function formatMiB(bytes: number): string {
  return `${(bytes / MIB).toFixed(1)} MiB`;
}

function TooLargeViewer({ doc }: UnreadViewerProps) {
  const T = useProjectStrings().editor;
  return <EditorNotice text={T.tooLarge(formatMiB(doc.size))} />;
}

function NotUtf8Viewer() {
  const T = useProjectStrings().editor;
  return <EditorNotice text={T.notUtf8} />;
}

export const UNREAD_VIEWERS: Readonly<
  Record<UnreadViewerProps["doc"]["status"], ComponentType<UnreadViewerProps>>
> = {
  tooLarge: TooLargeViewer,
  notUtf8: NotUtf8Viewer,
};
