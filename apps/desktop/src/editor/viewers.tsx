// 뷰어 종류 표(12-m1-plan M1-09): 종류(viewer-kinds.ts) → 그 종류를 그리는 컴포넌트. 에디터 탭(editor-panel.tsx)은 문서를 읽은 뒤
// 이 표에서 뷰어를 골라 그린다. 탭이 뷰어를 덮어쓰면(`EditorTabParams.viewer`, M1-23 「텍스트로 열기」 · 「hex 로 열기」) 그
// 종류를 쓴다. 읽는 중 · 실패는 탭이 그리고, 뷰어는 읽힌 문서만 받는다.
//
// 뷰어를 더하거나 바꾸는 법: 컴포넌트를 `ViewerProps` 로 만들고 아래 표의 그 종류 한 줄을 바꾼다. 내용을 어디서 받는지는
// viewer-kinds.ts 의 `VIEWER_READS` 다 — `text` 뷰어는 `doc.live`(여러 탭이 나누는 편집 중인 한 벌, live-text.ts)를, `self` 뷰어는
// 탭의 경로를 받아 필요한 범위를 직접 읽는다(preview-host.tsx).
//
// 텍스트로 읽지 않은 문서(바이너리 · 이미지)를 「텍스트로 열기」 하면 그 탭이 앞에서 50 MiB 까지 직접 읽은 읽기 전용 텍스트를
// 보인다(raw-text-viewer.tsx). 열지 않은 문서(`tooLarge`, 결정 M1-D9)는 `UNREAD_VIEWERS` 가 그린다 — 크기로 열지 않은 것은
// [앞부분 1 MiB 보기](「텍스트로 열기」 와 같다) · [hex 로 보기] · 외부 도구, 일반 파일이 아닌 것(FIFO · 장치)은 외부 도구만.

import type { IDockviewPanelProps } from "dockview-react";
import type { ComponentType } from "react";

import { useProjectStrings } from "../layout/strings";
import type { DocumentState } from "./documents";
import { type EditorTabParams, setTabViewer } from "./editor-tabs";
import { HexViewer } from "./hex-viewer";
import { ImageViewer } from "./image-viewer";
import { HEAD_PREVIEW_BYTES, READ_ONLY_MAX } from "./limits";
import { MarkdownViewer } from "./markdown-viewer";
import { EditorNotice } from "./notice";
import { RawTextViewer } from "./raw-text-viewer";
import { TextViewer } from "./text-viewer";
import type { ViewerKind } from "./viewer-kinds";

interface TabViewerProps<D> {
  /** 탭(dockview 패널)의 api. 패널 id · 보임 여부 · params 바꾸기를 쓴다. */
  panel: IDockviewPanelProps["api"];
  /** 탭 params(정규화한 경로 · 워크트리 · 탭의 언어 덮어쓰기 등). */
  params: EditorTabParams;
  doc: D;
}

export type ViewerProps = TabViewerProps<Extract<DocumentState, { status: "text" | "external" }>>;

export type UnreadViewerProps = TabViewerProps<Extract<DocumentState, { status: "tooLarge" }>>;

/** 스스로 읽는 뷰어(hex)가 받는 것: 읽힌 문서와 크기로 열지 않은 문서 둘 다. */
export type SelfReadProps = TabViewerProps<
  Extract<DocumentState, { status: "text" | "external" | "tooLarge" }>
>;

/** 텍스트 문서는 텍스트 뷰어, 텍스트로 읽지 않은 문서(「텍스트로 열기」)는 그 탭이 읽은 읽기 전용 텍스트. */
function TextOrRawViewer(props: ViewerProps) {
  const T = useProjectStrings().editor;
  if (props.doc.status === "text") return <TextViewer {...props} />;
  return (
    <RawTextViewer
      panel={props.panel}
      params={props.params}
      limit={READ_ONLY_MAX}
      notice={T.binaryAsText}
    />
  );
}

export const VIEWERS: Readonly<Record<ViewerKind, ComponentType<ViewerProps>>> = {
  text: TextOrRawViewer,
  markdown: MarkdownViewer,
  image: ImageViewer,
  hex: HexViewer,
};

const MIB = 1024 * 1024;

/** 바이트를 MiB 로(소수 한 자리). */
export function formatMiB(bytes: number): string {
  return `${(bytes / MIB).toFixed(1)} MiB`;
}

function TooLargeViewer(props: UnreadViewerProps) {
  const T = useProjectStrings().editor;
  const { panel, params, doc } = props;
  if (doc.reason === "special") return <EditorNotice text={T.special} />;
  if (params.viewer === "hex") return <HexViewer {...props} />;
  if (params.viewer === "text")
    return (
      <RawTextViewer
        panel={panel}
        params={params}
        limit={HEAD_PREVIEW_BYTES}
        notice={T.headNotice(formatMiB(HEAD_PREVIEW_BYTES), formatMiB(doc.size))}
      />
    );
  return (
    <EditorNotice
      text={T.tooLarge(formatMiB(doc.size))}
      actions={[
        {
          name: "head",
          label: T.showHead(formatMiB(HEAD_PREVIEW_BYTES)),
          run: () => setTabViewer(panel, params, "text"),
        },
        { name: "hex", label: T.showHex, run: () => setTabViewer(panel, params, "hex") },
      ]}
    />
  );
}

export const UNREAD_VIEWERS: Readonly<
  Record<UnreadViewerProps["doc"]["status"], ComponentType<UnreadViewerProps>>
> = {
  tooLarge: TooLargeViewer,
};
