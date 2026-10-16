// 프로젝트 창의 도킹 레이아웃(docs/design/10-ui.md §2.4): 왼쪽 · 오른쪽 · 하단 툴 윈도우와 가운데 에디터 영역.
// 툴 윈도우는 dockview 의 edge group 에, 에디터 영역은 가운데 grid 에 둔다. 지금은 모두 자리만 잡은 빈 패널이다.
//
// 저장본은 `{ version, layout: dockview.toJSON() }` 이다. 읽을 수 없거나(JSON · 버전 · dockview 검사) 이 앱이 모르는
// 패널이 들어 있으면 버리고 기본 레이아웃으로 연다. 모르는 패널을 그대로 열면 dockview 는 오류 없이 빈 탭을 만든다.
//
// 에디터 영역은 닫을 수 없다. 툴 윈도우 바에 에디터 자리가 없어, 닫히면 되돌릴 UI 가 없다. 탭에서 닫기를 빼고
// (`EDITOR_TAB`), dockview 가 따로 가진 닫기 경로(포커스된 탭의 Delete · Backspace, api 의 close)는 `keepEditor` 가
// 가운데에 다시 넣는다 — dockview 에는 닫기를 거부하는 이벤트가 없다. 에디터가 없는 저장본(닫기를 막기 전에 저장된 것)도
// 복원한 뒤 가운데에 다시 넣는다.

import type { DockviewApi, SerializedDockview } from "dockview-react";

export type Dock = "left" | "right" | "bottom";

export interface PanelSpec {
  id: string;
  title: string;
  /** 기본 레이아웃에서 놓이는 곳. 닫았다가 툴 윈도우 바로 다시 열 때도 이리로 돌아온다. */
  home: Dock | "editor";
  component: PanelComponent;
  /** 탭 머리의 종류. project-window.tsx 의 TAB_COMPONENTS 키. 없으면 dockview 기본 탭(닫기 있음). */
  tab?: typeof EDITOR_TAB;
}

/** 닫기(× · 가운데 클릭)가 없는 탭. */
export const EDITOR_TAB = "noClose";

/** 패널 내용의 종류. project-window.tsx 의 COMPONENTS 가 이 이름마다 컴포넌트를 둔다. */
export type PanelComponent = "toolWindow" | "editorArea";

// 10-ui §2.4 「구역별 구성」 표의 툴 윈도우. 순서가 기본 레이아웃의 탭 순서이고, 각 무리의 첫 패널이 처음에 보인다.
export const PANELS: readonly PanelSpec[] = [
  { id: "editor", title: "에디터", home: "editor", component: "editorArea", tab: EDITOR_TAB },
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
  { id: "queryResults", title: "쿼리 결과", home: "bottom", component: "toolWindow" },
  { id: "searchResults", title: "검색 결과", home: "bottom", component: "toolWindow" },
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

// 가운데 grid 의 그룹. 없으면 만든다. 위치 없이 더한 패널은 활성 그룹(edge group 일 수 있다)으로 가므로 늘 그룹을 집어 준다.
function centreGroup(api: DockviewApi) {
  return api.groups.find((g) => g.api.location.type === "grid") ?? api.addGroup();
}

function addPanel(api: DockviewApi, panel: PanelSpec, inactive: boolean) {
  const group = panel.home === "editor" ? centreGroup(api) : ensureDock(api, panel.home);
  return api.addPanel({
    id: panel.id,
    title: panel.title,
    component: panel.component,
    tabComponent: panel.tab,
    position: { referenceGroup: group.id },
    inactive,
  });
}

export function buildDefaultLayout(api: DockviewApi) {
  api.clear();
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
    // 탭 종류는 저장본이 아니라 이 앱의 패널 정의를 따른다 — 닫기를 막기 전의 저장본도 에디터 탭에 닫기가 없게.
    const normalized = panel as { tabComponent?: string };
    if (known.tab) normalized.tabComponent = known.tab;
    else delete normalized.tabComponent;
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
      if (!api.getPanel("editor")) addPanel(api, spec("editor")!, false);
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
 * 레이아웃이 바뀔 때마다 저장한다. 크기 조절처럼 잇달아 오는 변경은 한 프레임(`frameMs`) 안의 것만 모아 한 번 쓴다 —
 * localStorage 쓰기는 동기이고 저장본이 작아 매 프레임 써도 싸다. 오래 모으면 그 사이 앱을 끄는 순간의 변경이 사라진다.
 * 창을 닫으면(pagehide) 기다리던 저장을 바로 쓴다. ⚠ 앱 종료(메뉴 Quit · ⌘Q)에는 pagehide · beforeunload · unload ·
 * visibilitychange 가 오지 않는다(WKWebView 에서 리스너가 localStorage 에 남긴 기록이 재실행 뒤 없었다). 종료 쪽은
 * 저장 타이머가 종료보다 먼저 도는 데 기댄다. 잰 것: 툴 윈도우 바 버튼을 CGEvent 로 누르고 System Events 로 메뉴 Quit 을
 * 누른 뒤 재실행해 활성 탭을 읽었더니, 누른 직후(약 20ms 뒤) Quit 을 요청해도 변경이 남았다. 메뉴 Quit 은 요청부터
 * 종료까지 수백 ms 걸려 그 사이 타이머가 돈다 — 그래서 이 방법으로는 300ms 동안 모아 쓰는 방식도 남아 둘을 가르지 못한다.
 * 종료가 이보다 빠른 경로(강제 종료 · 크래시)의 마지막 한 프레임은 지켜지지 않는다.
 */
export function persistLayout(
  api: DockviewApi,
  storage: LayoutStorage,
  {
    frameMs = 16,
    win = window,
  }: { frameMs?: number; win?: Pick<EventTarget, "addEventListener" | "removeEventListener"> } = {},
) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const flush = () => {
    if (timer === undefined) return;
    clearTimeout(timer);
    timer = undefined;
    saveLayout(api, storage);
  };
  const changed = api.onDidLayoutChange(() => {
    // 첫 변경에서 잡은 시점에 쓴다. 뒤 변경이 시점을 미루지 않게 — 손을 떼지 않고 이어지는 크기 조절도 한 프레임마다 저장된다.
    timer ??= setTimeout(flush, frameMs);
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

/** 에디터 패널이 어떤 경로로든 지워지면 가운데에 다시 넣는다. */
export function keepEditor(api: DockviewApi) {
  return api.onDidRemovePanel((panel) => {
    if (panel.id !== "editor") return;
    // 옮기기 · clear 뒤 기본 레이아웃처럼 같은 흐름 안에서 다시 생기는 경우가 있어, 흐름이 끝난 뒤에 본다.
    queueMicrotask(() => {
      if (!api.getPanel("editor")) addPanel(api, spec("editor")!, false);
    });
  });
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
