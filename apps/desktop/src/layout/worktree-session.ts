// 프로젝트 창의 워크트리별 탭 묶음(결정 M1-D19, 04-features.md 「워크트리 경로 처리 (F2-5)」): 열린 탭 · 활성 탭 · 커서 · 스크롤 ·
// 트리 펼침을 활성 워크트리(`ActiveWorktree.top`, 메인 체크아웃은 `undefined`)마다 프로젝트 상태(project-state.ts)에 둔다.
//
// - 창이 받은 열기 요청(src/project-open.ts)을 여기서 차례로 처리한다. 요청의 워크트리가 지금 것과 다르면 바꾼다: 지금 묶음을 저장 →
//   탭을 모두 닫는다(에디터의 닫기 길 `closeTabs` — 미저장이 있으면 M1-13 의 [저장] [저장 안 함] [취소] 를 묻는다. 취소하면 바꾸지
//   않고 요청도 버린다) → 활성 워크트리를 바꾼다 → 그 워크트리의 묶음을 연다 → 요청의 파일을 연다.
// - 창이 처음 열리면 활성 워크트리의 묶음을 연다. 첫 요청이 다른 워크트리를 가리키면 그 묶음만 연다(쓸데없이 열었다 닫지 않게).
// - 탭은 에디터의 공개 열기 길(`workspace.open`)로 다시 연다. 열기 전에 파일이 아직 있는지 보고(`exists`), 사라진 파일의 탭은 조용히
//   건너뛴다 — 열기 실패 알림을 띄우지 않는다. 분할(에디터 그룹 여럿)은 되살리지 않고 한 그룹에 차례로 연다.
// - 탭은 그 탭을 연 워크트리로 다시 연다(`TabState.worktree`) — ⌘E · Search Everywhere 는 활성 워크트리를 바꾸지 않고 다른
//   워크트리의 파일을 연다(M1-19). 그 워크트리가 사라졌으면 열기 실패 알림이 뜬다(M1-19 의 「열 수 없음」). 묶음의 워크트리로 바꿔
//   열지 않는다.
// - 저장은 바뀔 때마다(탭 열기 · 닫기 · 활성 · 이동, 커서, 트리 펼침) 잠깐 모아(`SAVE_DELAY_MS`) 묶음을 다시 만든다. 스크롤은
//   이벤트가 없어 그때 · 창을 닫을 때(`flush`) 읽는다. 바꾸는 · 여는 동안에는 만들지 않는다 — 닫히는 탭이 저장본을 비우지 않게.
// - 탭의 커서 · 맨 위 줄은 뷰가 있으면 뷰에서, 없으면(아직 읽는 중) 마지막으로 알던 값이다.

import { EditorView } from "@codemirror/view";
import type { DockviewApi, IDockviewPanel } from "dockview-react";

import { editorPanels, isEditorPanel, tabParams, togglePinned } from "../editor/editor-tabs";
import { type Cursor, cursorOf, type EditorWorkspace } from "../editor/editor-workspace";
import type { FileTreeModel } from "../file-tree/tree-model";
import { requestedWorktree } from "../open-with/active-worktree";
import type { ProjectOpen } from "../project-open";
import type { ProjectState, TabState, WorktreeKey, WorktreeState } from "./project-state";

export const SAVE_DELAY_MS = 200;

/** 뷰의 맨 위 줄(1부터). */
export function topLine(view: EditorView): number {
  const block = view.lineBlockAtHeight(view.scrollDOM.scrollTop);
  return view.state.doc.lineAt(block.from).number;
}

/** `top` 줄을 화면 맨 위로. */
function scrollToLine(view: EditorView, top: number) {
  const doc = view.state.doc;
  const line = doc.line(Math.min(Math.max(1, top), doc.lines));
  view.dispatch({ effects: EditorView.scrollIntoView(line.from, { y: "start" }) });
}

export interface WorktreeSessionDeps {
  api: DockviewApi;
  workspace: EditorWorkspace;
  state: ProjectState;
  tree: FileTreeModel;
  /** 지금 트리가 어느 워크트리의 것인가(file-tree/project-view.tsx 의 `treeScope`). 다르면 트리 펼침을 저장하지 않는다. */
  treeWorktree(): WorktreeKey;
  activeWorktree(): WorktreeKey;
  setActiveWorktree(top: WorktreeKey): void;
  /**
   * 그 워크트리에 파일이 아직 있는가. Host 가 「없다」 고 답했을 때만 `false` 다 — 워크트리가 사라진 것 같은 그 밖의 실패는 `true`
   * 로 두어 열기 길이 알리게 한다(머리 주석).
   */
  exists(path: string, worktree: WorktreeKey): Promise<boolean>;
  log?(message: string): void;
  delayMs?: number;
  /** 뷰가 생길 때까지 기다리는 한 걸음(시험이 바꾼다). */
  nextFrame?: () => Promise<void>;
  /**
   * 앞 세션이 받은 요청을 다 처리할 때까지(앞 세션의 `idle()`). StrictMode 는 effect 를 해제했다 다시 걸어 세션이 둘 생긴다 — 앞
   * 세션이 붙자마자 받은 요청을 마저 처리한 뒤에 이 세션이 시작해야 순서가 섞이지 않는다.
   */
  after?: Promise<void>;
}

export function createWorktreeSession(deps: WorktreeSessionDeps) {
  const { api, workspace, state, tree } = deps;
  const delayMs = deps.delayMs ?? SAVE_DELAY_MS;
  const nextFrame =
    deps.nextFrame ??
    (() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())));
  /** 화면에 있는 묶음. 아직 아무것도 열지 않았으면 `undefined`. */
  let shown: { key: WorktreeKey } | undefined;
  /** 바꾸는 · 여는 중. 그동안은 저장하지 않는다. */
  let busy = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let disposed = false;
  /** 탭(패널 id) → 마지막으로 알던 커서 · 맨 위 줄. */
  const known = new Map<string, { cursor?: Cursor; top?: number }>();
  const queue: ProjectOpen[] = [];
  let chain: Promise<void> = deps.after ?? Promise.resolve();

  function snapshot(key: WorktreeKey): Omit<WorktreeState, "usedAt"> {
    const panels = api.groups
      .filter((g) => g.api.location.type === "grid")
      .flatMap((g) => g.panels)
      .filter(isEditorPanel);
    const active = workspace.activeTab();
    const tabs = panels.map((panel): TabState => {
      const { path, worktree, preview, pinned } = tabParams(panel);
      const view = workspace.view(panel.id);
      const last = known.get(panel.id);
      const cursor = view ? cursorOf(view) : last?.cursor;
      const top = view ? topLine(view) : last?.top;
      return {
        path,
        ...(worktree !== key && { worktree: worktree ?? null }),
        ...(preview && { preview: true }),
        ...(pinned && { pinned: true }),
        ...(cursor && { cursor }),
        ...(top !== undefined && top > 1 && { top }),
      };
    });
    const root = tree.getRoot();
    const treeState =
      root !== undefined && deps.treeWorktree() === key
        ? { root, expanded: tree.expandedPaths() }
        : state.bundle(key)?.tree;
    const index = active ? panels.indexOf(active) : -1;
    return {
      tabs,
      ...(index >= 0 && { active: index }),
      ...(treeState && { tree: treeState }),
    };
  }

  function save() {
    if (timer !== undefined) {
      clearTimeout(timer);
      timer = undefined;
    }
    if (!shown || busy > 0 || disposed) return;
    state.setBundle(shown.key, snapshot(shown.key));
  }

  function changed() {
    if (!shown || busy > 0) return;
    timer ??= setTimeout(save, delayMs);
  }

  async function whenView(panelId: string): Promise<EditorView | undefined> {
    for (let i = 0; i < 600 && !disposed && api.getPanel(panelId); i++) {
      const view = workspace.view(panelId);
      if (view) return view;
      await nextFrame();
    }
    return undefined;
  }

  /** 그 워크트리의 묶음을 연다(지금 탭이 없다고 본다). */
  async function restore(key: WorktreeKey) {
    busy++;
    try {
      const bundle = state.bundle(key);
      if (bundle?.tree) void tree.restoreExpanded(bundle.tree.root, bundle.tree.expanded);
      const opened: (IDockviewPanel | undefined)[] = [];
      // 해제된 뒤에도 끝까지 연다 — 받은 요청은 이 세션이 마저 처리한다(`after`).
      for (const tab of bundle?.tabs ?? []) {
        const worktree = tab.worktree === undefined ? key : (tab.worktree ?? undefined);
        if (!(await deps.exists(tab.path, worktree))) {
          deps.log?.(`skipped a saved tab, the file is gone: ${tab.path}`);
          opened.push(undefined);
          continue;
        }
        const panel = await workspace.open({
          path: tab.path,
          worktree,
          preview: tab.preview,
          line: tab.cursor?.line,
          column: tab.cursor?.column,
        });
        opened.push(panel);
        if (!panel) continue;
        known.set(panel.id, { cursor: tab.cursor, top: tab.top });
        if (tab.pinned && !tabParams(panel).pinned) togglePinned(panel);
        const top = tab.top;
        if (top !== undefined)
          void whenView(panel.id).then((view) => view && scrollToLine(view, top));
      }
      const active = bundle?.active !== undefined ? opened[bundle.active] : undefined;
      if (active && api.getPanel(active.id)) {
        active.api.setActive();
        workspace.focus(active.id);
      }
    } finally {
      busy--;
      shown = { key };
    }
  }

  /** 활성 워크트리를 바꾼다(머리 주석). 바꿨으면 `true`, 미저장 확인을 취소했으면 `false`. */
  async function switchTo(key: WorktreeKey): Promise<boolean> {
    if (shown && shown.key === key) return true;
    save();
    busy++;
    try {
      if (!(await workspace.closeTabs(editorPanels(api)))) return false;
      known.clear();
      deps.setActiveWorktree(key);
    } finally {
      busy--;
    }
    await restore(key);
    save();
    return true;
  }

  async function handle(open: ProjectOpen) {
    const next = requestedWorktree(open);
    if (next.change) {
      if (!(await switchTo(next.top))) {
        deps.log?.("kept the worktree: closing its tabs was cancelled");
        return;
      }
    } else if (!shown) await restore(deps.activeWorktree());
    if (!open.file) return;
    const panel = await workspace.open({
      path: open.file,
      worktree: open.worktree,
      line: open.line,
      column: open.col,
    });
    // 뷰가 생기기 전(문서를 읽는 중)에 저장해도 요청의 줄이 남게.
    if (panel && open.line !== undefined)
      known.set(panel.id, { cursor: { line: open.line, column: open.col ?? 1 } });
  }

  function enqueue(step: () => Promise<void>) {
    chain = chain
      .then(step)
      .catch((e: unknown) => console.warn("[session] worktree session step failed", e));
  }

  const subscriptions = [
    api.onDidAddPanel(changed),
    api.onDidRemovePanel((panel) => {
      known.delete(panel.id);
      changed();
    }),
    api.onDidActivePanelChange(changed),
    api.onDidLayoutChange(changed),
  ];
  const offCursor = workspace.onCursor((panelId, cursor) => {
    known.set(panelId, { ...known.get(panelId), cursor });
    changed();
  });
  const offTree = tree.subscribe(changed);

  return {
    /**
     * 창의 열기 요청을 받기 시작한다(`inbox.connect` 를 넘긴다). 처음에 활성 워크트리의 묶음을 연다 — 붙자마자 받은 첫 요청이
     * 다른 워크트리면 그것만 연다. 해제 함수를 돌려준다.
     */
    start(connect: (onOpen: (open: ProjectOpen) => void) => () => void): () => void {
      enqueue(async () => {
        const first = queue[0] && requestedWorktree(queue[0]);
        if (!shown && !(first?.change && first.top !== deps.activeWorktree()))
          await restore(deps.activeWorktree());
      });
      return connect((open) => {
        queue.push(open);
        enqueue(() => handle(queue.shift()!));
      });
    },

    /** 기다리던 저장을 지금 한다(창을 닫기 전). */
    flush: save,

    /** 지금 처리 중인 요청이 끝날 때까지(시험). */
    idle: (): Promise<void> => chain,

    /** 구독을 걷는다. 이미 받은 요청은 마저 처리하고, 그 뒤에 한 번 저장하고 멈춘다(`after`). */
    dispose() {
      for (const s of subscriptions) s.dispose();
      offCursor();
      offTree();
      chain = chain
        .then(() => {
          save();
          disposed = true;
        })
        .catch((e: unknown) => {
          disposed = true;
          console.warn("[session] could not save the tab set", e);
        });
    },
  };
}

export type WorktreeSession = ReturnType<typeof createWorktreeSession>;
