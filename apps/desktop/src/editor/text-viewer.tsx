// 텍스트 뷰어: 탭 하나에 EditorView 하나. 텍스트 · undo 기록은 문서 모델의 `LiveText`(live-text.ts) 한 벌이고, 커서 · 선택 ·
// 스크롤은 뷰마다 따로다 — 그래서 두 그룹이 같은 파일을 보여도 한쪽 스크롤이 다른 쪽을 움직이지 않고, 한쪽에서 고친 것은 다른
// 쪽에 곧바로 보인다. 확장은 조립 지점(extensions/index.ts)에서 받는다.
//
// 뷰는 문서(`LiveText`)가 바뀔 때만 다시 만든다 — 편집 · 저장 · 디스크에서 다시 읽은 내용은 문서가 뷰에 트랜잭션으로 옮긴다.
// 뷰를 없앨 때 그 탭의 선택 · 스크롤을 문서에 맡기고(`attach` 의 해제), 같은 탭의 다음 뷰가 그 자리에서 시작한다(탭을 다른
// 그룹으로 옮겨 내용이 다시 마운트될 때). 기능이 보는 정보(`EditorFeatureContext` — 언어 · 외관 · 읽기 전용 등)가 바뀌면 같은 뷰를
// `reconfigureEditor` 로 다시 구성한다. context 에 값을 더할 때는 `useFeatureContext` 만 고친다.
//
// dockview 는 가려진 탭의 내용 요소를 DOM 에서 떼어 두므로 다시 보일 때 스크롤 위치가 0 으로 돌아간다. 보이는 동안의 스크롤을
// 기억해 두었다가 다시 보일 때 되돌린다.
//
// 디스크 쪽이 이 문서와 다르면(documents.ts 의 `disk`) 위에 배너를 띄운다(disk-banner.tsx). 고칠 수 없는 문서(5 MiB 초과 · UTF-8 이
// 아닌 인코딩)는 그 까닭을 위 막대에 적는다.
//
// 찾기 · 바꾸기(⌘F / ⌘R)는 CodeMirror 의 위쪽 패널 자리에 찾기 막대(find-bar.tsx)를 portal 로 그린다 — 막대가 창의 문자열 · 테마
// context 를 그대로 쓴다.

import type { StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";

import { useProjectStrings } from "../layout/strings";
import { useTheme } from "../theme/theme-switcher";
import { CompareView, DiskBanner } from "./disk-banner";
import type { TextDocumentState } from "./documents";
import { useEditorFont } from "./editor-font";
import type { EditorTabParams } from "./editor-tabs";
import { type EditorWorkspace, useEditorWorkspace } from "./editor-workspace";
import { type EditorFeatureContext, editorExtensions, reconfigureEditor } from "./extensions";
import { FindBar, findPanel } from "./find-bar";
import { useDocLanguage } from "./language";
import type { LiveText } from "./live-text";
import type { ViewerProps } from "./viewers";

/** 탭 · 문서 · 창에서 기능이 볼 정보. 값이 같으면 같은 객체다(바뀔 때만 다시 구성한다). */
function useFeatureContext(
  workspace: EditorWorkspace,
  panelId: string,
  params: EditorTabParams,
  text: TextDocumentState | undefined,
  createFindPanel: EditorFeatureContext["createFindPanel"],
): EditorFeatureContext | undefined {
  const appearance = useTheme((s) => s.appearance);
  const font = useEditorFont();
  const language = useDocLanguage(params.path);
  const live = text?.live;
  const judgment = text?.judgment;
  const readOnly = text?.readOnly;
  return useMemo(
    () =>
      live && judgment && readOnly !== undefined
        ? {
            panelId,
            path: params.path,
            judgment,
            readOnly,
            language,
            appearance,
            onCursor: (cursor) => workspace.reportCursor(panelId, cursor),
            history: live,
            syntax: workspace.syntax,
            font,
            createFindPanel,
          }
        : undefined,
    [
      workspace,
      panelId,
      params.path,
      live,
      judgment,
      readOnly,
      language,
      appearance,
      font,
      createFindPanel,
    ],
  );
}

export function TextViewer({ panel, params, doc }: ViewerProps) {
  const T = useProjectStrings().editor;
  const workspace = useEditorWorkspace();
  const host = useRef<HTMLDivElement>(null);
  const text = doc.status === "text" ? doc : undefined;
  const live: LiveText | undefined = text?.live;
  const { path } = params;
  const [find, setFind] = useState<{ view: EditorView; dom: HTMLElement }>();
  const createFindPanel = useCallback(
    (view: EditorView) =>
      findPanel(view, setFind, (dom) => setFind((f) => (f?.dom === dom ? undefined : f))),
    [],
  );
  const context = useFeatureContext(workspace, panel.id, params, text, createFindPanel);
  const latest = useRef(context);
  latest.current = context;
  // 지금 뷰와 그 뷰를 마지막으로 구성한 context.
  const current = useRef<{ view: EditorView; context: EditorFeatureContext }>(undefined);
  // 배너의 [비교]: 디스크가 바뀐 동안만 연다.
  const [comparing, setComparing] = useState(false);
  const diskRead = text?.disk.kind === "changed" ? text.disk.read : undefined;
  useEffect(() => {
    if (!diskRead) setComparing(false);
  }, [diskRead]);
  const compare = comparing && live && diskRead?.content.kind === "text" ? diskRead : undefined;
  // 고칠 수 없는 까닭(결정 M1-D9 · M1-D12). UTF-8 이 아닌 텍스트는 Host 가 짐작한 인코딩으로 푼 것이다.
  const encoding = text?.judgment.binary === false ? text.judgment.encoding : null;
  const readOnlyNotice = text?.judgment.lossy
    ? T.readOnlyLossy
    : encoding && encoding !== "UTF-8"
      ? T.readOnlyEncoding(encoding)
      : text?.judgment.sizeClass === "readOnly"
        ? T.readOnlyLarge
        : undefined;

  useEffect(() => {
    const parent = host.current;
    const context = latest.current;
    if (!live || !parent || !context) return;
    const made = live.createState(panel.id, editorExtensions(context));
    const view = new EditorView({
      parent,
      state: made.state,
      dispatchTransactions: (transactions, view) => live.dispatch(transactions, view),
    });
    current.current = { view, context };
    let scroll: StateEffect<unknown> | undefined = made.scroll;
    if (scroll) view.dispatch({ effects: scroll });
    const detach = live.attach(panel.id, view, () => scroll);
    const unregister = workspace.registerView(panel.id, view);

    const started = workspace.takeOpenedAt(panel.id);
    if (started !== undefined) {
      // 첫 그림 = 뷰를 만든 커밋을 담은 프레임(두 번째 rAF, src/perf-mark.tsx 와 같은 기준).
      requestAnimationFrame(() =>
        requestAnimationFrame(() =>
          workspace.log?.(
            `first paint ${path}: ${view.state.doc.lines} lines, ${(performance.now() - started).toFixed(1)} ms`,
          ),
        ),
      );
    }

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
      detach();
      if (current.current?.view === view) current.current = undefined;
      view.destroy();
    };
  }, [workspace, panel, path, live]);

  // 같은 뷰를 새 context 로 다시 구성한다. 뷰를 방금 만든 렌더면 이미 그 context 라 할 것이 없다.
  useEffect(() => {
    const now = current.current;
    if (!now || !context || now.context === context) return;
    now.view.dispatch(reconfigureEditor(context));
    now.context = context;
  }, [context]);

  return (
    <div className="flex h-full min-h-0 flex-col bg-editor-background">
      {readOnlyNotice && (
        <div
          role="status"
          data-read-only-notice=""
          className="flex-none border-b border-ui-border bg-ui-panel px-2 py-0.5 text-[12px] text-ui-muted-foreground"
        >
          {readOnlyNotice}
        </div>
      )}
      {text && text.disk.kind !== "same" && (
        <DiskBanner
          path={path}
          worktree={params.worktree}
          doc={text}
          comparing={compare !== undefined}
          onCompare={() => setComparing((c) => !c)}
        />
      )}
      {compare && live && <CompareView mine={live.text} disk={compare} />}
      {find && createPortal(<FindBar view={find.view} host={find.dom} />, find.dom)}
      {/* 텍스트 선택을 막는 body 의 user-select: none(styles.css)을 에디터 안에서는 푼다. 비교하는 동안은 가린다(뷰는 남는다). */}
      <div
        ref={host}
        data-testid="text-viewer"
        className={`min-h-0 flex-1 overflow-hidden select-text ${compare ? "hidden" : ""}`}
      />
    </div>
  );
}
