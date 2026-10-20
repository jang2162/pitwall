// 에디터 탭의 내용(dockview 패널 `EDITOR_PANEL`): 탭의 파일을 문서 모델에서 잡고(탭의 워크트리를 접근 범위로), 읽는 중 · 실패를
// 안내하거나 뷰어 표(viewers.tsx)에서 고른 뷰어를 그린다. 탭의 파일은 패널 params 의 `path` 다(editor-tabs.ts) — 탭이 사는 동안
// 바뀌지 않는다. 다른 params(뷰어 · 언어 덮어쓰기)는 바뀔 수 있고, 바뀌면 뷰를 다시 만들지 않고 뷰어가 따라간다.
//
// 뷰어는 탭의 덮어쓰기(`params.viewer`)와 문서의 종류로 고른다(viewer-kinds.ts `chooseViewer`).

import type { IDockviewPanelProps } from "dockview-react";

import { useProjectStrings } from "../layout/strings";
import { useDocument } from "./documents";
import type { EditorTabParams } from "./editor-tabs";
import { useEditorWorkspace } from "./editor-workspace";
import { EditorNotice } from "./notice";
import { chooseViewer } from "./viewer-kinds";
import { UNREAD_VIEWERS, VIEWERS } from "./viewers";

export function EditorPanel({ api, params }: IDockviewPanelProps<EditorTabParams>) {
  const T = useProjectStrings().editor;
  const workspace = useEditorWorkspace();
  const doc = useDocument(workspace.documents, params.path, params.worktree);
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
          case "notUtf8": {
            const Viewer = UNREAD_VIEWERS[doc.status];
            return <Viewer panel={api} params={params} doc={doc} />;
          }
          case "text":
          case "external": {
            const Viewer = VIEWERS[chooseViewer(doc, params.viewer)];
            return <Viewer panel={api} params={params} doc={doc} />;
          }
        }
      })()}
    </div>
  );
}
