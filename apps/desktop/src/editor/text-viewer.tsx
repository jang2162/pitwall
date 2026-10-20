// 텍스트 뷰어: 탭 하나에 EditorView 하나. 문서의 텍스트(`Text`)는 문서 모델의 한 벌을 나누고, 커서 · 스크롤은 뷰마다 따로다 —
// 그래서 두 그룹이 같은 파일을 보여도 한쪽 스크롤이 다른 쪽을 움직이지 않는다. 확장은 조립 지점(extensions/index.ts)에서 받는다.
//
// dockview 는 가려진 탭의 내용 요소를 DOM 에서 떼어 두므로 다시 보일 때 스크롤 위치가 0 으로 돌아간다. 보이는 동안의 스크롤을
// 기억해 두었다가 다시 보일 때 되돌린다.

import { EditorState, type StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { useEffect, useRef } from "react";

import { useProjectStrings } from "../layout/strings";
import { editorExtensions } from "./extensions";
import { useEditorWorkspace } from "./editor-workspace";
import type { ViewerProps } from "./viewers";

export function TextViewer({ panel, path, doc }: ViewerProps) {
  const T = useProjectStrings().editor;
  const workspace = useEditorWorkspace();
  const host = useRef<HTMLDivElement>(null);
  const text = doc.status === "text" ? doc : undefined;

  useEffect(() => {
    const parent = host.current;
    if (!text || !parent) return;
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: text.text,
        extensions: editorExtensions({ path, judgment: text.judgment, readOnly: text.readOnly }),
      }),
    });
    const unregister = workspace.registerView(panel.id, view);

    const started = workspace.takeOpenedAt(panel.id);
    if (started !== undefined) {
      // 첫 그림 = 뷰를 만든 커밋을 담은 프레임(두 번째 rAF, src/perf-mark.tsx 와 같은 기준).
      requestAnimationFrame(() =>
        requestAnimationFrame(() =>
          workspace.log?.(
            `first paint ${path}: ${text.text.lines} lines, ${(performance.now() - started).toFixed(1)} ms`,
          ),
        ),
      );
    }

    let scroll: StateEffect<unknown> | undefined;
    const remember = () => {
      if (panel.isVisible && view.dom.isConnected) scroll = view.scrollSnapshot();
    };
    view.scrollDOM.addEventListener("scroll", remember);
    const shown = panel.onDidVisibilityChange(({ isVisible }) => {
      if (isVisible && scroll) view.dispatch({ effects: scroll });
    });
    return () => {
      shown.dispose();
      view.scrollDOM.removeEventListener("scroll", remember);
      unregister();
      view.destroy();
    };
  }, [workspace, panel, path, text]);

  return (
    <div className="flex h-full min-h-0 flex-col bg-editor-background">
      {text?.judgment.sizeClass === "readOnly" && (
        <div
          role="status"
          className="flex-none border-b border-ui-border bg-ui-panel px-2 py-0.5 text-[12px] text-ui-muted-foreground"
        >
          {T.readOnlyLarge}
        </div>
      )}
      {/* 텍스트 선택을 막는 body 의 user-select: none(styles.css)을 에디터 안에서는 푼다. */}
      <div
        ref={host}
        data-testid="text-viewer"
        className="min-h-0 flex-1 overflow-hidden select-text"
      />
    </div>
  );
}
