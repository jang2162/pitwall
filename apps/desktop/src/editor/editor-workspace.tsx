// 창 하나의 에디터: 문서 모델(documents.ts), 탭(editor-tabs.ts), 탭마다의 뷰(EditorView)와 커서. 열기 경로와 「활성 탭」 문맥이
// 여기 있다.
//
// 열기 경로(`open`): 요청 경로를 `fs.stat` 으로 정규화해(심링크를 따라간 실제 경로 — 문서 모델 · 탭의 키) 탭을 열고, 줄 · 열이
// 있으면 커서를 그 자리에 두고 그 줄을 화면 가운데로 가져온다. 뷰가 아직 없으면(문서를 읽는 중) 뷰가 생길 때 둔다(`pending`).
// 탭이 남아 있는데 뷰가 사라지면(StrictMode 의 다시 마운트 · 문서를 다시 읽음) 그 뷰의 커서를 `pending` 에 돌려놓아 다음 뷰가
// 같은 자리에서 시작한다.
// 창이 받은 열기 요청(`project://open`, src/project-open.ts)은 고정 탭으로, 파일 트리 클릭(M1-14)은 미리보기로 연다.
//
// 「활성 탭」: 활성 패널이 에디터 탭이면 그것, 아니면(툴 윈도우를 보는 중) 마지막으로 쓴 에디터 탭. 외부 도구 문맥(파일 · 커서
// 줄 · 열, openWith.run)과 탭 액션(⌘W 등)이 이것을 쓴다.

import { EditorSelection } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { FsStat } from "@pitwall/rpc-client";
import type { DockviewApi, IDockviewPanel } from "dockview-react";
import { createContext, type ReactNode, useContext } from "react";

import type { DocumentStore } from "./documents";
import { isEditorPanel, openTab, type RecentTabs, tabParams, trackRecentTabs } from "./editor-tabs";

/** 1부터 센 줄 · 열(열은 글자 단위). */
export interface Cursor {
  line: number;
  column: number;
}

export interface OpenRequest {
  /** 절대 경로. 정규화 전이어도 된다. */
  path: string;
  line?: number;
  column?: number;
  preview?: boolean;
}

/** 커서를 줄 · 열에 두고 그 줄을 화면 가운데로. 범위를 넘으면 끝으로 붙인다. */
export function placeCursor(view: EditorView, { line, column }: Cursor) {
  const doc = view.state.doc;
  const at = doc.line(Math.min(Math.max(1, Math.trunc(line)), doc.lines));
  const pos = at.from + Math.min(Math.max(0, Math.trunc(column) - 1), at.length);
  view.dispatch({
    selection: EditorSelection.cursor(pos),
    effects: EditorView.scrollIntoView(pos, { y: "center" }),
  });
}

export function cursorOf(view: EditorView): Cursor {
  const head = view.state.selection.main.head;
  const line = view.state.doc.lineAt(head);
  return { line: line.number, column: head - line.from + 1 };
}

export interface EditorWorkspaceDeps {
  documents: DocumentStore;
  stat(path: string): Promise<FsStat>;
  /** 열지 못한 이유(요청 경로, 오류 문장). 창이 알림으로 보인다. */
  onOpenFailed(path: string, message: string): void;
  /** 진단 로그(첫 그림까지 걸린 시간 등). */
  log?(message: string): void;
}

export function createEditorWorkspace(deps: EditorWorkspaceDeps) {
  let api: DockviewApi | undefined;
  let recent: RecentTabs | undefined;
  const views = new Map<string, EditorView>();
  const pending = new Map<string, Cursor>();
  /** 탭을 열기 시작한 시각(performance.now). 첫 그림을 재는 데 쓴다. */
  const openedAt = new Map<string, number>();

  const recentOrder = () => recent?.order() ?? [];

  function activeTab(): IDockviewPanel | undefined {
    if (!api) return undefined;
    if (isEditorPanel(api.activePanel)) return api.activePanel;
    for (const id of recentOrder()) {
      const panel = api.getPanel(id);
      if (isEditorPanel(panel)) return panel;
    }
    return undefined;
  }

  function moveCursor(panelId: string, cursor: Cursor) {
    const view = views.get(panelId);
    if (view) {
      placeCursor(view, cursor);
      view.focus();
    } else pending.set(panelId, cursor);
  }

  return {
    documents: deps.documents,
    log: deps.log,

    /** 도킹 영역이 준비되면 붙는다. 해제 함수를 돌려준다. */
    attach(next: DockviewApi): () => void {
      api = next;
      recent = trackRecentTabs(next);
      const tracked = recent;
      const removed = next.onDidRemovePanel((panel) =>
        queueMicrotask(() => {
          if (next.getPanel(panel.id)) return;
          pending.delete(panel.id);
          openedAt.delete(panel.id);
        }),
      );
      return () => {
        removed.dispose();
        tracked.dispose();
        if (api === next) {
          api = undefined;
          recent = undefined;
        }
      };
    },

    api: () => api,
    recent: recentOrder,
    activeTab,

    /** 탭의 뷰를 등록한다. 기다리던 커서가 있으면 둔다. */
    registerView(panelId: string, view: EditorView): () => void {
      views.set(panelId, view);
      const cursor = pending.get(panelId);
      pending.delete(panelId);
      if (cursor) {
        placeCursor(view, cursor);
        view.focus();
      }
      return () => {
        if (views.get(panelId) !== view) return;
        views.delete(panelId);
        if (api?.getPanel(panelId)) pending.set(panelId, cursorOf(view));
      };
    },

    view: (panelId: string) => views.get(panelId),

    /** 탭을 연 시각을 꺼낸다(한 번만). 텍스트 뷰어가 첫 그림을 잴 때 쓴다. */
    takeOpenedAt(panelId: string): number | undefined {
      const at = openedAt.get(panelId);
      openedAt.delete(panelId);
      return at;
    },

    async open(request: OpenRequest): Promise<IDockviewPanel | undefined> {
      const started = performance.now();
      let stat: FsStat;
      try {
        stat = await deps.stat(request.path);
      } catch (e) {
        deps.onOpenFailed(request.path, e instanceof Error ? e.message : String(e));
        return undefined;
      }
      if (!api) return undefined;
      // 폴더는 탭으로 열지 않는다. 열기 요청(CLI)은 Rust 가 이미 거르고, 파일 트리는 폴더를 펼친다.
      if (stat.kind === "dir" || stat.target === "dir") return undefined;
      const { panel, created } = openTab(api, {
        path: stat.path,
        preview: request.preview,
        recent: recentOrder(),
      });
      if (created) openedAt.set(panel.id, started);
      if (request.line !== undefined)
        moveCursor(panel.id, { line: request.line, column: request.column ?? 1 });
      return panel;
    },

    /** 활성 탭의 파일과 커서(외부 도구 문맥). 탭이 없으면 `undefined`. 뷰가 없는 탭(이미지 등)은 커서 없이. */
    activeContext(): { file: string; cursor?: Cursor } | undefined {
      const panel = activeTab();
      if (!panel) return undefined;
      const view = views.get(panel.id);
      return { file: tabParams(panel).path, cursor: view ? cursorOf(view) : undefined };
    },
  };
}

export type EditorWorkspace = ReturnType<typeof createEditorWorkspace>;

const EditorWorkspaceContext = createContext<EditorWorkspace | undefined>(undefined);

export function EditorWorkspaceProvider({
  workspace,
  children,
}: {
  workspace: EditorWorkspace;
  children: ReactNode;
}) {
  return <EditorWorkspaceContext value={workspace}>{children}</EditorWorkspaceContext>;
}

export function useEditorWorkspace(): EditorWorkspace {
  const workspace = useContext(EditorWorkspaceContext);
  if (!workspace) throw new Error("EditorWorkspaceProvider 밖에서 에디터를 썼다");
  return workspace;
}
