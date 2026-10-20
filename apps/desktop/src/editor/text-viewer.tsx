// 텍스트 뷰어: 탭 하나에 EditorView 하나. 문서의 텍스트(`Text`)는 문서 모델의 한 벌을 나누고, 커서 · 스크롤은 뷰마다 따로다 —
// 그래서 두 그룹이 같은 파일을 보여도 한쪽 스크롤이 다른 쪽을 움직이지 않는다. 확장은 조립 지점(extensions/index.ts)에서 받는다.
//
// 뷰는 문서(읽은 텍스트)가 바뀔 때만 다시 만든다. 기능이 보는 정보(`EditorFeatureContext` — 언어 · 외관 · 읽기 전용 등)가 바뀌면
// 같은 뷰를 `reconfigureEditor` 로 다시 구성한다. context 에 값을 더할 때는 `useFeatureContext` 만 고친다.
//
// dockview 는 가려진 탭의 내용 요소를 DOM 에서 떼어 두므로 다시 보일 때 스크롤 위치가 0 으로 돌아간다. 보이는 동안의 스크롤을
// 기억해 두었다가 다시 보일 때 되돌린다.

import { EditorState, type StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { useEffect, useMemo, useRef } from "react";

import { useProjectStrings } from "../layout/strings";
import { useTheme } from "../theme/theme-switcher";
import type { DocumentState } from "./documents";
import type { EditorTabParams } from "./editor-tabs";
import { type EditorWorkspace, useEditorWorkspace } from "./editor-workspace";
import { type EditorFeatureContext, editorExtensions, reconfigureEditor } from "./extensions";
import type { ViewerProps } from "./viewers";

type TextDocument = Extract<DocumentState, { status: "text" }>;

/** 탭 · 문서 · 창에서 기능이 볼 정보. 값이 같으면 같은 객체다(바뀔 때만 다시 구성한다). */
function useFeatureContext(
  workspace: EditorWorkspace,
  panelId: string,
  params: EditorTabParams,
  text: TextDocument | undefined,
): EditorFeatureContext | undefined {
  const appearance = useTheme((s) => s.appearance);
  const language = params.language !== undefined ? params.language : (text?.language ?? null);
  return useMemo(
    () =>
      text && {
        panelId,
        path: params.path,
        judgment: text.judgment,
        readOnly: text.readOnly,
        language,
        appearance,
        onCursor: (cursor) => workspace.reportCursor(panelId, cursor),
      },
    [workspace, panelId, params.path, text, language, appearance],
  );
}

export function TextViewer({ panel, params, doc }: ViewerProps) {
  const T = useProjectStrings().editor;
  const workspace = useEditorWorkspace();
  const host = useRef<HTMLDivElement>(null);
  const text = doc.status === "text" ? doc : undefined;
  const { path } = params;
  const context = useFeatureContext(workspace, panel.id, params, text);
  const latest = useRef(context);
  latest.current = context;
  // 지금 뷰와 그 뷰를 마지막으로 구성한 context.
  const current = useRef<{ view: EditorView; context: EditorFeatureContext }>(undefined);

  useEffect(() => {
    const parent = host.current;
    const context = latest.current;
    if (!text || !parent || !context) return;
    const view = new EditorView({
      parent,
      state: EditorState.create({ doc: text.text, extensions: editorExtensions(context) }),
    });
    current.current = { view, context };
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
      if (current.current?.view === view) current.current = undefined;
      view.destroy();
    };
  }, [workspace, panel, path, text]);

  // 같은 뷰를 새 context 로 다시 구성한다. 뷰를 방금 만든 렌더면 이미 그 context 라 할 것이 없다.
  useEffect(() => {
    const now = current.current;
    if (!now || !context || now.context === context) return;
    now.view.dispatch(reconfigureEditor(context));
    now.context = context;
  }, [context]);

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
