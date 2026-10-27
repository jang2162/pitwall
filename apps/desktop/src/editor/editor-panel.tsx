// 에디터 탭의 내용(dockview 패널 `EDITOR_PANEL`): 탭의 파일을 문서 모델에서 잡고(탭의 워크트리를 접근 범위로), 읽는 중 · 실패를
// 안내하거나 뷰어 표(viewers.tsx)에서 고른 뷰어를 그린다. 탭의 파일은 패널 params 의 `path` 다(editor-tabs.ts) — 탭이 사는 동안
// 바뀌지 않는다. 다른 params(뷰어 · 언어 덮어쓰기)는 바뀔 수 있고, 바뀌면 뷰를 다시 만들지 않고 뷰어가 따라간다.
//
// 뷰어는 탭의 덮어쓰기(`params.viewer`)와 문서의 종류로 고른다(viewer-kinds.ts `chooseViewer`).
//
// params 에 `git` 이 있으면 파일이 아니라 git 문서(커밋 상세 · 비교 · 리비전 파일 · diff)다 — 문서 모델을 잡지 않고 그 문서를
// 그린다(src/git/documents).

import type { IDockviewPanelProps } from "dockview-react";

import { GitDocumentPanel } from "../git/documents/document-panel";
import { isGitDocumentSpec } from "../git/documents/spec";
import { useProjectStrings } from "../layout/strings";
import { useDocument } from "./documents";
import type { EditorTabParams } from "./editor-tabs";
import { useEditorWorkspace } from "./editor-workspace";
import { EditorNotice } from "./notice";
import { chooseViewer } from "./viewer-kinds";
import { UNREAD_VIEWERS, VIEWERS } from "./viewers";

export function EditorPanel(props: IDockviewPanelProps<EditorTabParams>) {
  const { api, params } = props;
  const T = useProjectStrings().editor;
  if (params.git === undefined) return <FilePanel {...props} />;
  // git 문서는 글자 에디터가 아니다 — 에디터 키(`editorFocus`)를 받지 않는다. diff 뷰 안은 `diffFocus`(src/git/diff/diff-view.tsx).
  return (
    <div data-keymap-context="gitDocument" data-editor-tab={api.id} className="h-full">
      {isGitDocumentSpec(params.git) ? (
        <GitDocumentPanel params={params} spec={params.git} />
      ) : (
        <EditorNotice text={T.failed("git")} openWith={false} />
      )}
    </div>
  );
}

function FilePanel({ api, params }: IDockviewPanelProps<EditorTabParams>) {
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
          case "tooLarge": {
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
