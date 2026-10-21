// 창 하나의 에디터: 문서 모델(documents.ts), 탭(editor-tabs.ts), 탭마다의 뷰(EditorView)와 커서. 열기 경로와 「활성 탭」 문맥이
// 여기 있다.
//
// 열기 경로(`open`): 요청 경로를 `fs.stat` 으로 정규화해(심링크를 따라간 실제 경로 — 문서 모델 · 탭의 키) 탭을 열고, 줄 · 열이
// 있으면 커서를 그 자리에 두고 그 줄을 화면 가운데로 가져온다. 뷰가 아직 없으면(문서를 읽는 중) 뷰가 생길 때 둔다(`pending`).
// 탭이 남아 있는데 뷰가 사라지면(그룹 사이로 옮김 · StrictMode 의 다시 마운트) 그 탭의 선택 · 스크롤은 문서가 맡는다
// (live-text.ts `attach`) — 다음 뷰가 같은 자리에서 시작한다.
// 창이 받은 열기 요청(`project://open`, src/project-open.ts)은 고정 탭으로, 파일 트리 클릭(M1-14)은 미리보기로 연다. 요청은 자기
// 워크트리를 싣고 오고, 정규화 · 문서 읽기 · 탭이 그 워크트리를 쓴다 — 대기열에 쌓였다가 나중에 열리는 요청도 창의 그때 활성
// 워크트리가 아니라 요청을 받은 때의 워크트리로 읽힌다.
//
// 「활성 탭」: 활성 패널이 에디터 탭이면 그것, 아니면(툴 윈도우를 보는 중) 마지막으로 쓴 에디터 탭. 외부 도구 문맥(워크트리 ·
// 파일 · 커서 줄 · 열, openWith.run)과 탭 액션(⌘W 등)이 이것을 쓴다.
//
// 문서의 수명(M1-13): 에디터 탭(패널)이 있는 동안 그 파일의 문서를 잡는다(`onDidAddPanel` · `onDidRemovePanel`). 탭의 내용
// 컴포넌트가 다시 마운트돼도(그룹 사이로 옮김 · StrictMode) 문서가 놓이지 않아 편집 · undo 기록이 남는다. 그룹 사이로 옮기는 것은
// 지우기가 아니다(dockview 8 은 `onDidMovePanel` 만 낸다).
//
// 닫기 확인: 탭을 닫는 길(⌘W · 탭 닫기 액션 · 다른 탭 닫기 · 탭의 × · 가운데 클릭)은 모두 `closeTabs` 를 지난다. 그 문서를 보는
// 마지막 탭을 닫는데 미저장 편집이 있으면 [저장] [저장 안 함] [취소] 를 묻는다. 창 닫기 · 앱 종료는 `confirmWindowClose` 다.
// 미리보기 탭의 문서에 미저장 편집이 생기면 그 탭을 보통 탭으로 바꾼다 — 다음 미리보기가 편집을 조용히 닫지 않게(10-ui §5.4
// 「편집 시작 → 고정 탭」).

import { EditorSelection } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { FsStat } from "@pitwall/rpc-client";
import type { DockviewApi, IDockviewPanel } from "dockview-react";
import { createContext, type ReactNode, useContext } from "react";

import type { DocumentStore } from "./documents";
import {
  editorPanels,
  isEditorPanel,
  keepTab,
  openTab,
  type RecentTabs,
  tabParams,
  trackRecentTabs,
} from "./editor-tabs";

/** 1부터 센 줄 · 열(열은 글자 단위). */
export interface Cursor {
  line: number;
  column: number;
}

export interface OpenRequest {
  /** 절대 경로. 정규화 전이어도 된다. */
  path: string;
  /** 요청의 워크트리 꼭대기(`ProjectOpen.worktree`). 없으면 메인 체크아웃. */
  worktree?: string;
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

/** 미저장 확인의 답. */
export type UnsavedAnswer = "save" | "discard" | "cancel";

export interface EditorWorkspaceDeps {
  documents: DocumentStore;
  stat(path: string, worktree: string | undefined): Promise<FsStat>;
  /** 열지 못한 이유(요청 경로, 오류 문장). 창이 알림으로 보인다. */
  onOpenFailed(path: string, message: string): void;
  /** 저장하지 못한 이유(경로, 오류 문장). 충돌은 여기 오지 않는다 — 탭의 배너가 보인다. */
  onSaveFailed(path: string, message: string): void;
  /** 닫으면 잃는 미저장 편집이 있다(경로). 창이 대화상자로 묻는다. */
  confirmUnsaved(paths: readonly string[]): Promise<UnsavedAnswer>;
  /** 「저장」 을 골랐는데 다 저장하지 못해 닫지 않았다. */
  onCloseAborted?(): void;
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
  const cursorListeners = new Set<(panelId: string, cursor: Cursor) => void>();
  /** 뷰가 포커스를 가진 채 사라진 탭. 그 탭의 다음 뷰가 포커스를 받는다(그룹 사이로 옮김 · StrictMode). */
  const refocus = new Set<string>();
  /** 탭(패널 id) → 그 탭이 잡은 문서를 놓는 함수. */
  const holds = new Map<string, { path: string; release(): void }>();
  /** 미저장 확인이 떠 있다. 그동안의 닫기 요청은 버린다. */
  let confirming = false;

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

  function hold(panel: IDockviewPanel) {
    if (!isEditorPanel(panel) || holds.has(panel.id)) return;
    const { path, worktree } = tabParams(panel);
    holds.set(panel.id, { path, release: deps.documents.acquire(path, worktree) });
  }

  function unhold(panelId: string) {
    const held = holds.get(panelId);
    if (!held) return;
    holds.delete(panelId);
    const doc = deps.documents.get(held.path);
    if (doc?.status === "text") doc.live.forget(panelId);
    held.release();
  }

  /** 미저장 편집을 묻고 답대로 한다. 닫아도 되면 `true`. */
  async function settleUnsaved(paths: readonly string[]): Promise<boolean> {
    if (paths.length === 0) return true;
    if (confirming) return false;
    confirming = true;
    let answer: UnsavedAnswer;
    try {
      answer = await deps.confirmUnsaved(paths);
    } finally {
      confirming = false;
    }
    if (answer === "cancel") return false;
    if (answer === "discard") return true;
    const saved = await Promise.all(paths.map((path) => save(path)));
    if (saved.every(Boolean)) return true;
    deps.onCloseAborted?.();
    return false;
  }

  /** 문서를 저장한다. 저장했거나 쓸 것이 없으면 `true`. 실패는 알리고, 충돌은 탭의 배너가 보인다. */
  async function save(path: string): Promise<boolean> {
    const outcome = await deps.documents.save(path);
    if (outcome.kind === "failed") deps.onSaveFailed(path, outcome.message);
    return outcome.kind === "saved" || outcome.kind === "unchanged";
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
      for (const panel of editorPanels(next)) hold(panel);
      const added = next.onDidAddPanel(hold);
      const removed = next.onDidRemovePanel((panel) => {
        pending.delete(panel.id);
        openedAt.delete(panel.id);
        refocus.delete(panel.id);
        unhold(panel.id);
      });
      // 미리보기 탭의 문서가 미저장이 되면 보통 탭으로.
      const kept = deps.documents.store.subscribe(({ docs }, before) => {
        for (const panel of editorPanels(next)) {
          const { path, preview } = tabParams(panel);
          const doc = docs[path];
          if (preview && doc !== before.docs[path] && doc?.status === "text" && doc.dirty)
            keepTab(panel);
        }
      });
      return () => {
        kept();
        added.dispose();
        removed.dispose();
        // 지우면서 돌아도 된다(Map 의 반복은 지운 항목을 건너뛴다).
        for (const id of holds.keys()) unhold(id);
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
      } else if (refocus.delete(panelId)) view.focus();
      return () => {
        if (views.get(panelId) !== view) return;
        views.delete(panelId);
        if (api?.getPanel(panelId) && view.hasFocus) refocus.add(panelId);
      };
    },

    view: (panelId: string) => views.get(panelId),

    /** 탭의 뷰에 포커스를 둔다. 뷰가 아직 없으면 생길 때 둔다(⌘\ 로 나눈 새 탭). */
    focus(panelId: string) {
      const view = views.get(panelId);
      if (view) view.focus();
      else refocus.add(panelId);
    },

    /**
     * 탭들을 닫는다. 그 문서를 보는 마지막 탭인데 미저장 편집이 있으면 먼저 묻는다 — [취소] 면 아무것도 닫지 않는다. 닫았으면
     * `true`.
     */
    async closeTabs(panels: readonly IDockviewPanel[]): Promise<boolean> {
      const closing = new Set(panels.map((p) => p.id));
      const open = api ? editorPanels(api) : [];
      const lost = [...new Set(panels.map((p) => tabParams(p).path))].filter((path) => {
        const doc = deps.documents.get(path);
        if (doc?.status !== "text" || !doc.live.dirty) return false;
        return !open.some((p) => !closing.has(p.id) && tabParams(p).path === path);
      });
      if (!(await settleUnsaved(lost))) return false;
      for (const panel of panels) if (api?.getPanel(panel.id)) panel.api.close();
      return true;
    },

    /** 창을 닫기 전(창의 닫기 단추 · ⌘W · 앱 종료). 미저장 편집이 있으면 묻는다. 닫아도 되면 `true`. */
    confirmWindowClose: (): Promise<boolean> => settleUnsaved(deps.documents.unsaved()),

    /** 탭의 문서를 저장한다(`⌘S`). */
    save: (panel: IDockviewPanel): Promise<boolean> => save(tabParams(panel).path),

    /** 미저장 편집이 있는 문서를 모두 저장한다. */
    async saveAll(): Promise<boolean> {
      const saved = await Promise.all(deps.documents.unsaved().map((path) => save(path)));
      return saved.every(Boolean);
    },

    /** 뷰의 커서가 바뀌었다(extensions/cursor-report.ts 가 부른다). */
    reportCursor(panelId: string, cursor: Cursor) {
      for (const listener of cursorListeners) listener(panelId, cursor);
    },

    /** 탭의 커서가 바뀔 때마다 듣는다(상태바의 커서 줄:열, M1-18). 해제 함수를 돌려준다. */
    onCursor(listener: (panelId: string, cursor: Cursor) => void): () => void {
      cursorListeners.add(listener);
      return () => cursorListeners.delete(listener);
    },

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
        stat = await deps.stat(request.path, request.worktree);
      } catch (e) {
        deps.onOpenFailed(request.path, e instanceof Error ? e.message : String(e));
        return undefined;
      }
      if (!api) return undefined;
      // 폴더는 탭으로 열지 않는다. 열기 요청(CLI)은 Rust 가 이미 거르고, 파일 트리는 폴더를 펼친다.
      if (stat.kind === "dir" || stat.target === "dir") return undefined;
      const { panel, created } = openTab(api, {
        path: stat.path,
        worktree: request.worktree,
        preview: request.preview,
        recent: recentOrder(),
      });
      if (created) openedAt.set(panel.id, started);
      if (request.line !== undefined)
        moveCursor(panel.id, { line: request.line, column: request.column ?? 1 });
      return panel;
    },

    /**
     * 활성 탭의 워크트리 · 파일 · 커서(외부 도구 문맥). 탭이 없으면 `undefined`. 뷰가 없는 탭(이미지 등)은 커서 없이. `worktree`
     * 가 `undefined` 면 그 탭은 메인 체크아웃의 것이다.
     */
    activeContext(): { worktree?: string; file: string; cursor?: Cursor } | undefined {
      const panel = activeTab();
      if (!panel) return undefined;
      const view = views.get(panel.id);
      const { path, worktree } = tabParams(panel);
      return { worktree, file: path, cursor: view ? cursorOf(view) : undefined };
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
