// 에디터 탭의 내용(dockview 패널 `EDITOR_PANEL`): 탭의 파일을 문서 모델에서 잡고, 읽기 상태를 안내하거나 뷰어 종류 표(viewers.tsx)
// 에서 고른 뷰어를 그린다. 탭의 파일은 패널 params 의 `path` 다(editor-tabs.ts) — 탭이 사는 동안 바뀌지 않는다.

import type { IDockviewPanelProps } from "dockview-react";

import { useProjectStrings } from "../layout/strings";
import { useDocument } from "./documents";
import type { EditorTabParams } from "./editor-tabs";
import { useEditorWorkspace } from "./editor-workspace";
import { EditorNotice } from "./notice";
import { VIEWERS } from "./viewers";

const MIB = 1024 * 1024;

/** 바이트를 MiB 로(소수 한 자리). */
export function formatMiB(bytes: number): string {
  return `${(bytes / MIB).toFixed(1)} MiB`;
}

export function EditorPanel({ api, params }: IDockviewPanelProps<EditorTabParams>) {
  const T = useProjectStrings().editor;
  const workspace = useEditorWorkspace();
  const { path } = params;
  const doc = useDocument(workspace.documents, path);
  // 키맵 컨텍스트의 포커스 층(src/keymap/context.ts): 에디터 안의 키 바인딩이 `when = "editorFocus"` 로 쓴다.
  return (
    <div data-keymap-context="editorFocus" data-editor-tab={api.id} className="h-full">
      {(() => {
        switch (doc.status) {
          case "loading":
            return <EditorNotice text={T.loading} openWith={false} />;
          case "failed":
            return <EditorNotice text={T.failed(doc.message)} />;
          case "tooLarge":
            return <EditorNotice text={T.tooLarge(formatMiB(doc.size))} />;
          case "notUtf8":
            return <EditorNotice text={T.notUtf8} />;
          case "text":
          case "external": {
            const Viewer = VIEWERS[doc.kind];
            return <Viewer panel={api} path={path} doc={doc} />;
          }
        }
      })()}
    </div>
  );
}
