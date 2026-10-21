// 이 창에서 최근에 본 파일(⌘E 최근 파일, ⌘⇧O 의 「최근 연 파일 먼저」). 에디터 탭이 활성이 될 때마다 앞에 둔다 — 닫은 탭의 파일도
// 남는다(탭 전환기의 순서 editor-tabs.ts `trackRecentTabs` 는 열린 탭만 갖는다). 이름을 바꾸거나 지운 파일의 옛 경로도 남는다 —
// 고르면 열기가 실패를 알린다. 창의 메모리에만 둔다: 창을 닫으면 잊는다.
// 다시 시작한 뒤의 복원은 상태 복원(12-m1-plan M1-25)이 정한다.

import type { DockviewApi } from "dockview-react";
import { createStore } from "zustand/vanilla";

import { editorPanels, isEditorPanel, tabParams } from "../editor/editor-tabs";

/** 기억하는 파일 수. */
export const RECENT_FILES_LIMIT = 50;

export interface RecentFile {
  /** 탭의 경로(정규화한 실제 경로). */
  readonly path: string;
  /** 탭을 연 워크트리 꼭대기. 없으면 메인 체크아웃. */
  readonly worktree?: string;
}

export function createRecentFiles() {
  const store = createStore<{ files: readonly RecentFile[] }>()(() => ({ files: [] }));
  const remember = (file: RecentFile) =>
    store.setState(({ files }) => ({
      files: [file, ...files.filter((f) => f.path !== file.path)].slice(0, RECENT_FILES_LIMIT),
    }));
  return {
    store,
    remember,
    /** 앞이 최근. */
    list: (): readonly RecentFile[] => store.getState().files,
    /** 도킹 영역의 에디터 탭 활성화를 듣는다. 이미 열린 탭은 활성 탭이 맨 앞이 되게 넣는다. 해제 함수를 돌려준다. */
    track(api: DockviewApi): () => void {
      const add = (panel: Parameters<typeof isEditorPanel>[0]) => {
        if (!isEditorPanel(panel)) return;
        const { path, worktree } = tabParams(panel);
        if (path) remember({ path, ...(worktree !== undefined && { worktree }) });
      };
      for (const panel of editorPanels(api).reverse()) add(panel);
      add(api.activePanel);
      const activated = api.onDidActivePanelChange(({ panel }) => add(panel));
      return () => activated.dispose();
    },
  };
}

export type RecentFiles = ReturnType<typeof createRecentFiles>;
