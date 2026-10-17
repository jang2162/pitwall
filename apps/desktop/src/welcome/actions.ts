// 웰컴 창의 액션(10-ui §1 원칙 2 — 우클릭 메뉴 항목도 액션이라 액션 찾기 · 키로 닿는다). 기본 키는
// extensions/keymap-default/keymaps/default.toml.

import type { Action, Platform } from "@pitwall/keymap";
import type { Project } from "@pitwall/rpc-client";

import type { WelcomeStrings } from "./strings";

export function welcomeActions(deps: {
  strings: WelcomeStrings;
  platform: Platform;
  /** 목록에서 고른 프로젝트. */
  selected(): Project | undefined;
  /** 검색 칸이 비어 있는가. 비어 있지 않으면 ⌘⌫ 는 검색어를 지운다. */
  searchEmpty(): boolean;
  /** 레지스트리에 쓸 수 있는가(storage 배너 · 연결 전). */
  writable(): boolean;
  newProject(): void;
  openFolder(): Promise<void>;
  open(project: Project): unknown;
  reveal(project: Project): unknown;
  remove(project: Project): unknown;
}): Action[] {
  const T = deps.strings.actions;
  const hasSelection = () => deps.selected() !== undefined;
  const withSelected = (run: (p: Project) => unknown) => () => {
    const p = deps.selected();
    if (p) void run(p);
  };
  return [
    {
      id: "project.new",
      label: T.newProject,
      keywords: ["New Project"],
      enabled: deps.writable,
      run: deps.newProject,
    },
    {
      id: "project.openFolder",
      label: T.openFolder,
      keywords: ["Open Folder", "Open"],
      run: deps.openFolder,
    },
    {
      id: "project.open",
      label: T.open,
      keywords: ["Open Project"],
      enabled: hasSelection,
      run: withSelected(deps.open),
    },
    {
      id: "project.reveal",
      label: T.reveal(deps.platform),
      keywords: ["Reveal in Finder", "Show in Explorer"],
      enabled: hasSelection,
      run: withSelected(deps.reveal),
    },
    {
      id: "project.removeFromList",
      label: T.remove,
      keywords: ["Remove from List"],
      enabled: () => hasSelection() && deps.searchEmpty() && deps.writable(),
      run: withSelected(deps.remove),
    },
  ];
}

/** S3 생성 시트가 열려 있는 동안의 팝업 액션. */
export function createProjectActions(deps: {
  strings: WelcomeStrings;
  canSubmit(): boolean;
  submit(): unknown;
}): Action[] {
  return [
    {
      id: "project.create",
      label: deps.strings.actions.create,
      popup: true,
      enabled: deps.canSubmit,
      run: () => void deps.submit(),
    },
  ];
}
