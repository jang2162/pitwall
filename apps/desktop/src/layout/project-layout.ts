// 프로젝트 창의 도킹 레이아웃(docs/design/10-ui.md §2.4): 왼쪽 · 오른쪽 · 하단 툴 윈도우와 가운데 에디터 영역.
// 툴 윈도우는 dockview 의 edge group 에, 에디터 영역은 가운데 grid 에 둔다. 지금은 모두 자리만 잡은 빈 패널이다.
//
// 저장본은 `{ version, layout: dockview.toJSON() }` 이다. 읽을 수 없거나(JSON · 버전 · dockview 검사) 이 앱이 모르는
// 패널이 들어 있으면 버리고 기본 레이아웃으로 연다. 모르는 패널을 그대로 열면 dockview 는 오류 없이 빈 탭을 만든다.

import type { DockviewApi, SerializedDockview } from "dockview-react";

export type Dock = "left" | "right" | "bottom";

export interface PanelSpec {
  id: string;
  title: string;
  /** 기본 레이아웃에서 놓이는 곳. 닫았다가 툴 윈도우 바로 다시 열 때도 이리로 돌아온다. */
  home: Dock | "editor";
  component: PanelComponent;
}

/** 패널 내용의 종류. project-window.tsx 의 COMPONENTS 가 이 이름마다 컴포넌트를 둔다. */
export type PanelComponent = "toolWindow" | "editorArea";

// 10-ui §2.4 「구역별 구성」 표의 툴 윈도우. 순서가 기본 레이아웃의 탭 순서이고, 각 무리의 첫 패널이 처음에 보인다.
export const PANELS: readonly PanelSpec[] = [
  { id: "editor", title: "에디터", home: "editor", component: "editorArea" },
  { id: "project", title: "프로젝트", home: "left", component: "toolWindow" },
  { id: "commit", title: "커밋", home: "left", component: "toolWindow" },
  { id: "database", title: "DB 탐색기", home: "left", component: "toolWindow" },
  { id: "search", title: "검색", home: "left", component: "toolWindow" },
  { id: "structure", title: "구조", home: "left", component: "toolWindow" },
  { id: "run", title: "실행", home: "bottom", component: "toolWindow" },
  { id: "debug", title: "디버그", home: "bottom", component: "toolWindow" },
  { id: "problems", title: "문제", home: "bottom", component: "toolWindow" },
  { id: "terminal", title: "터미널", home: "bottom", component: "toolWindow" },
  { id: "git", title: "Git", home: "bottom", component: "toolWindow" },
  { id: "notifications", title: "알림", home: "right", component: "toolWindow" },
  { id: "agent", title: "에이전트 활동", home: "right", component: "toolWindow" },
];

const DOCK_SIZES: Record<Dock, number> = { left: 260, right: 260, bottom: 200 };
const DOCKS: readonly Dock[] = ["left", "bottom", "right"];

// 프로젝트별 기억(10-ui §2.4)은 프로젝트 모델(M0-09)이 생기면 키에 프로젝트 id 를 붙인다. 지금은 모든 프로젝트 창이 한 칸을 쓴다.
// 저장은 localStorage 다 — origin 별이라 `mise run dev` 의 개발 서버와 빌드본은 서로의 레이아웃을 보지 못한다.
export const LAYOUT_STORAGE_KEY = "pitwall.layout.project";
/** 저장 형식이나 패널 구성이 호환되지 않게 바뀌면 올린다. 다른 버전의 저장본은 버리고 기본 레이아웃으로 연다. */
export const LAYOUT_VERSION = 1;

export type LayoutStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;

export type RestoreResult = "restored" | "default" | "recovered";

function spec(id: string): PanelSpec | undefined {
  return PANELS.find((p) => p.id === id);
}

function edgeGroupId(dock: Dock) {
  return `dock-${dock}`;
}

function ensureDock(api: DockviewApi, dock: Dock) {
  return (
    api.getEdgeGroup(dock) ??
    api.addEdgeGroup(dock, { id: edgeGroupId(dock), initialSize: DOCK_SIZES[dock] })
  );
}

function addPanel(api: DockviewApi, panel: PanelSpec, inactive: boolean) {
  const position =
    panel.home === "editor" ? undefined : { referenceGroup: ensureDock(api, panel.home).id };
  return api.addPanel({
    id: panel.id,
    title: panel.title,
    component: panel.component,
    position,
    inactive,
  });
}

export function buildDefaultLayout(api: DockviewApi) {
  api.clear();
  // 에디터 영역을 먼저 둔다. 위치 없이 더한 패널은 활성 그룹으로 가므로, edge group 이 먼저 있으면 에디터가 그 안에 들어간다.
  addPanel(api, spec("editor")!, false);
  for (const dock of DOCKS) {
    PANELS.filter((p) => p.home === dock).forEach((panel, i) => addPanel(api, panel, i > 0));
  }
  api.getPanel("editor")?.api.setActive();
}

/** 저장본이 이 앱이 열 수 있는 것인지. 형식 검사는 dockview 의 `fromJSON` 이 한다 — 여기서는 버전과 패널만 본다. */
function readSaved(raw: string): SerializedDockview | undefined {
  let saved: unknown;
  try {
    saved = JSON.parse(raw);
  } catch {
    return undefined;
  }
  if (typeof saved !== "object" || saved === null) return undefined;
  const { version, layout } = saved as { version?: unknown; layout?: unknown };
  if (version !== LAYOUT_VERSION || typeof layout !== "object" || layout === null) return undefined;
  const panels = (layout as { panels?: unknown }).panels;
  if (typeof panels !== "object" || panels === null) return undefined;
  for (const [id, panel] of Object.entries(panels)) {
    const known = spec(id);
    const component = (panel as { contentComponent?: unknown } | null)?.contentComponent;
    if (!known || component !== known.component) return undefined;
  }
  return layout as SerializedDockview;
}

/**
 * 저장본으로 레이아웃을 연다. 없으면 기본 레이아웃(`default`), 깨졌으면 저장본을 지우고 기본 레이아웃(`recovered`).
 */
export function restoreLayout(api: DockviewApi, storage: LayoutStorage): RestoreResult {
  let raw: string | null;
  try {
    raw = storage.getItem(LAYOUT_STORAGE_KEY);
  } catch {
    raw = null;
  }
  if (raw === null) {
    buildDefaultLayout(api);
    return "default";
  }
  const layout = readSaved(raw);
  if (layout) {
    try {
      api.fromJSON(layout);
      return "restored";
    } catch (error) {
      console.warn("[layout] 저장된 레이아웃을 열지 못했다", error);
    }
  } else {
    console.warn("[layout] 저장된 레이아웃을 읽지 못했다");
  }
  try {
    storage.removeItem(LAYOUT_STORAGE_KEY);
  } catch {
    // 지우지 못해도 기본 레이아웃이 다음 저장에서 덮어쓴다.
  }
  buildDefaultLayout(api);
  return "recovered";
}

export function saveLayout(api: DockviewApi, storage: LayoutStorage) {
  try {
    storage.setItem(
      LAYOUT_STORAGE_KEY,
      JSON.stringify({ version: LAYOUT_VERSION, layout: api.toJSON() }),
    );
  } catch (error) {
    console.warn("[layout] 레이아웃을 저장하지 못했다", error);
  }
}

/**
 * 레이아웃이 바뀔 때마다 저장한다. 드래그 · 크기 조절은 변경을 잇달아 내므로 `delayMs` 동안 모아 한 번 쓴다.
 * 창이 닫히면 기다리던 저장을 바로 쓴다 — 마지막 변경 직후 앱을 끄면 그 변경이 사라지지 않게.
 */
export function persistLayout(
  api: DockviewApi,
  storage: LayoutStorage,
  {
    delayMs = 300,
    win = window,
  }: { delayMs?: number; win?: Pick<EventTarget, "addEventListener" | "removeEventListener"> } = {},
) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const flush = () => {
    if (timer === undefined) return;
    clearTimeout(timer);
    timer = undefined;
    saveLayout(api, storage);
  };
  const changed = api.onDidLayoutChange(() => {
    if (timer !== undefined) clearTimeout(timer);
    timer = setTimeout(flush, delayMs);
  });
  win.addEventListener("pagehide", flush);
  return {
    flush,
    dispose() {
      flush();
      changed.dispose();
      win.removeEventListener("pagehide", flush);
    },
  };
}

/** 툴 윈도우 바에서 고른 패널을 앞에 띄운다. 닫혀 있으면 제자리(`home`)에 다시 연다. */
export function showPanel(api: DockviewApi, id: string) {
  const existing = api.getPanel(id);
  if (existing) {
    existing.api.setActive();
    return;
  }
  const panel = spec(id);
  if (panel) addPanel(api, panel, false).api.setActive();
}
