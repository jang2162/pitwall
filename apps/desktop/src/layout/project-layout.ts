// 프로젝트 창의 도킹 레이아웃(docs/design/10-ui.md §2.4): 왼쪽 · 오른쪽 · 하단 툴 윈도우와 가운데 에디터 영역.
// 툴 윈도우는 dockview 의 edge group 에, 에디터 탭은 가운데 grid 의 그룹(에디터 그룹)에 둔다(결정 M1-D1, src/editor/editor-tabs.ts).
// 툴 윈도우는 아직 대부분 자리만 잡은 빈 패널이다.
//
// 저장본은 `{ version, layout: dockview.toJSON() }` 이다. 읽을 수 없거나(JSON · 버전 · dockview 검사) 이 앱이 모르는
// 패널이 들어 있으면 버리고 기본 레이아웃으로 연다. 모르는 패널을 그대로 열면 dockview 는 오류 없이 빈 탭을 만든다.
//
// 에디터 탭은 저장본에서 뺀다(`stripPanels`). 저장소가 프로젝트를 가리지 않는 한 칸이라(`LAYOUT_STORAGE_KEY`) 다른 프로젝트의
// 파일 탭이 열리게 된다 — 열린 탭의 복원은 프로젝트 · 워크트리별 상태(결정 M1-D19, M1-25)가 한다. 탭을 빼서 빈 grid 그룹은
// 복원한 뒤 지운다. 에디터 탭이 하나도 없으면 가운데는 빈 영역 표시(dockview watermark, project-window.tsx)다. 예전 저장본의
// 「에디터 영역」 자리 패널(`editor`)도 같이 빼고 읽는다.
//
// 툴 윈도우를 닫는 것은 숨기기다(IntelliJ 와 같다, 10-ui §1 원칙 1). 닫은 툴 윈도우와 닫을 때 있던 자리(`hidden`)를
// 저장본에 함께 두어, 재시작해도 숨은 채로 남고 다시 열면(툴 윈도우 바 · 열기 액션 `toolWindow.<id>`) 그 자리로 돌아온다.
// 모든 툴 윈도우에 열기 액션이 있어(src/keymap/actions.ts) 숨긴 것을 되돌릴 길이 늘 있다 — 하단처럼 툴 윈도우 바가 없는
// 자리도 액션 찾기(⌘⇧A)로 연다. 복원할 때 저장본에도 `hidden` 에도 없는 툴 윈도우는 제자리(`home`)에 채운다 — 새로 더한
// 툴 윈도우가 저장본이 있는 사용자에게도 나타나게.
//
// 탭 머리의 이름은 UI 언어를 따른다(layout/strings.ts). 패널은 이름 대신 id 를 달고 만들어지고, `applyPanelTitles` 가 지금
// 언어의 이름을 붙인다 — 저장본에 남은 이름(저장할 때의 언어)도 그것이 덮는다. 언어가 바뀌면 다시 부른다.

import type { DockviewApi, SerializedDockview } from "dockview-react";

import { EDITOR_PANEL } from "../editor/editor-tabs";

export type Dock = "left" | "right" | "bottom";

export interface PanelSpec {
  /** 이름은 이 id 로 찾는다(layout/strings.ts 의 `toolWindows`). */
  id: string;
  /** 기본 레이아웃에서 놓이는 곳. 닫았다가 툴 윈도우 바로 다시 열 때도 이리로 돌아온다. */
  home: Dock;
  component: PanelComponent;
}

/** 툴 윈도우 내용의 종류. project-window.tsx 의 COMPONENTS 가 이 이름마다 컴포넌트를 둔다(에디터 탭은 `EDITOR_PANEL`). */
export type PanelComponent = "toolWindow";

/** 예전 저장본의 「에디터 영역」 자리 패널. 읽을 때 뺀다(머리 주석). */
const LEGACY_EDITOR_AREA = { id: "editor", component: "editorArea" };

// 10-ui §2.4 「구역별 구성」 표의 툴 윈도우. 순서가 기본 레이아웃의 탭 순서이고, 각 무리의 첫 패널이 처음에 보인다.
export const PANELS: readonly PanelSpec[] = [
  { id: "project", home: "left", component: "toolWindow" },
  { id: "commit", home: "left", component: "toolWindow" },
  { id: "database", home: "left", component: "toolWindow" },
  { id: "search", home: "left", component: "toolWindow" },
  { id: "structure", home: "left", component: "toolWindow" },
  { id: "run", home: "bottom", component: "toolWindow" },
  { id: "debug", home: "bottom", component: "toolWindow" },
  { id: "problems", home: "bottom", component: "toolWindow" },
  { id: "terminal", home: "bottom", component: "toolWindow" },
  { id: "git", home: "bottom", component: "toolWindow" },
  { id: "queryResults", home: "bottom", component: "toolWindow" },
  { id: "searchResults", home: "bottom", component: "toolWindow" },
  { id: "notifications", home: "right", component: "toolWindow" },
  { id: "agent", home: "right", component: "toolWindow" },
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

/** 툴 윈도우가 있던 곳: 가장자리 dock 이거나 가운데 grid. */
export type Place = Dock | "centre";

// 숨긴 툴 윈도우 → 숨길 때 있던 곳. dockview api 마다 하나. 저장본의 `hidden` 이 이것이다.
const hiddenByApi = new WeakMap<DockviewApi, Map<string, Place>>();

function hiddenOf(api: DockviewApi): Map<string, Place> {
  let hidden = hiddenByApi.get(api);
  if (!hidden) {
    hidden = new Map();
    hiddenByApi.set(api, hidden);
  }
  return hidden;
}

/** 지금 숨어 있는 툴 윈도우(닫은 순서). */
export function hiddenPanels(api: DockviewApi): ReadonlyMap<string, Place> {
  return hiddenOf(api);
}

const PLACES: readonly Place[] = ["left", "right", "bottom", "centre"];

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

function placeOf(api: DockviewApi, groupId: string): Place {
  return (
    (["left", "right", "bottom"] as const).find((d) => api.getEdgeGroup(d)?.id === groupId) ??
    "centre"
  );
}

function addPanel(
  api: DockviewApi,
  panel: PanelSpec,
  inactive: boolean,
  place: Place = panel.home,
) {
  const group = place === "centre" ? centreGroup(api) : ensureDock(api, place);
  const added = api.addPanel({
    id: panel.id,
    // 이름은 `applyPanelTitles` 가 붙인다.
    title: panel.id,
    component: panel.component,
    position: { referenceGroup: group.id },
    inactive,
  });
  expandDock(added.group.api);
  return added;
}

// dockview 는 비게 된 edge 그룹을 접는다. 거기에 패널을 넣어도 접힌 채라 탭 머리만 보이므로 펼친다.
function expandDock(group: { location: { type: string }; isCollapsed(): boolean; expand(): void }) {
  if (group.location.type === "edge" && group.isCollapsed()) group.expand();
}

export function buildDefaultLayout(api: DockviewApi) {
  api.clear();
  hiddenOf(api).clear();
  for (const dock of DOCKS) {
    PANELS.filter((p) => p.home === dock).forEach((panel, i) => addPanel(api, panel, i > 0));
  }
}

/** 저장본의 `hidden`. 모르는 툴 윈도우 · 자리는 빼고 읽는다 — 이것 때문에 레이아웃 전체를 버리지 않는다. */
function readHidden(saved: unknown): Map<string, Place> {
  const out = new Map<string, Place>();
  const hidden = (saved as { hidden?: unknown }).hidden;
  if (typeof hidden !== "object" || hidden === null) return out;
  for (const [id, place] of Object.entries(hidden)) {
    const known = spec(id);
    if (known && PLACES.includes(place as Place)) out.set(id, place as Place);
  }
  return out;
}

interface SerializedGroupViews {
  views?: unknown;
  activeView?: unknown;
}

function stripViews(group: SerializedGroupViews | undefined, drop: ReadonlySet<string>) {
  if (!group || !Array.isArray(group.views)) return;
  group.views = (group.views as unknown[]).filter((v) => !drop.has(v as string));
  if (!(group.views as unknown[]).includes(group.activeView)) delete group.activeView;
}

function stripGrid(node: unknown, drop: ReadonlySet<string>) {
  const n = node as { type?: unknown; data?: unknown } | null;
  if (n?.type === "leaf") stripViews(n.data as SerializedGroupViews, drop);
  else if (n?.type === "branch" && Array.isArray(n.data))
    n.data.forEach((child) => stripGrid(child, drop));
}

/**
 * 저장본에서 패널을 뺀다(grid · floating · popout · edge group 의 탭 목록과 `panels`). 비게 된 그룹은 남는다 — 복원한 뒤
 * `removeEmptyGroups` 가 지운다. `layout` 을 고친다.
 */
export function stripPanels(layout: SerializedDockview, drop: ReadonlySet<string>) {
  if (drop.size === 0) return;
  for (const id of drop) delete layout.panels[id];
  stripGrid(layout.grid?.root, drop);
  for (const f of layout.floatingGroups ?? []) stripViews(f.data as SerializedGroupViews, drop);
  for (const p of layout.popoutGroups ?? []) stripViews(p.data as SerializedGroupViews, drop);
  for (const e of Object.values(layout.edgeGroups ?? {}))
    stripViews((e as { group?: SerializedGroupViews } | undefined)?.group, drop);
}

/** 탭이 없는 grid · floating 그룹을 지운다. edge group(툴 윈도우 dock)은 비어도 남긴다. */
function removeEmptyGroups(api: DockviewApi) {
  const empty = api.groups.filter((g) => g.panels.length === 0 && g.api.location.type !== "edge");
  for (const group of empty) api.removeGroup(group);
}

/** 저장본이 이 앱이 열 수 있는 것인지. 형식 검사는 dockview 의 `fromJSON` 이 한다 — 여기서는 버전과 패널만 본다. */
function readSaved(
  raw: string,
): { layout: SerializedDockview; hidden: Map<string, Place> } | undefined {
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
  const drop = new Set<string>();
  for (const [id, panel] of Object.entries(panels)) {
    const component = (panel as { contentComponent?: unknown } | null)?.contentComponent;
    if (
      component === EDITOR_PANEL ||
      (id === LEGACY_EDITOR_AREA.id && component === LEGACY_EDITOR_AREA.component)
    ) {
      drop.add(id);
      continue;
    }
    const known = spec(id);
    if (!known || component !== known.component) return undefined;
    // 탭 종류는 저장본이 아니라 이 앱의 패널 정의를 따른다 — 툴 윈도우는 dockview 기본 탭(닫기 있음)이다.
    delete (panel as { tabComponent?: string }).tabComponent;
  }
  stripPanels(layout as SerializedDockview, drop);
  return { layout: layout as SerializedDockview, hidden: readHidden(saved) };
}

// 저장본에 없고 숨기지도 않은 패널을 제자리에 넣는다(머리 주석). 뒤에 넣어 복원된 활성 탭을 바꾸지 않는다.
function fillMissingPanels(api: DockviewApi) {
  for (const panel of PANELS) {
    if (api.getPanel(panel.id) || hiddenOf(api).has(panel.id)) continue;
    addPanel(api, panel, true);
  }
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
  const saved = readSaved(raw);
  if (saved) {
    try {
      api.fromJSON(saved.layout);
      removeEmptyGroups(api);
      const hidden = hiddenOf(api);
      hidden.clear();
      // 저장본에 있는 패널이 `hidden` 에도 있으면(손으로 고친 저장본 등) 보이는 쪽을 믿는다.
      for (const [id, place] of saved.hidden) if (!api.getPanel(id)) hidden.set(id, place);
      fillMissingPanels(api);
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
      JSON.stringify({
        version: LAYOUT_VERSION,
        layout: api.toJSON(),
        hidden: Object.fromEntries(hiddenOf(api)),
      }),
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

/** 탭 머리에 `titles`(툴 윈도우 id → 지금 언어의 이름)를 붙인다. 열린 패널과 앞으로 더해질 패널 모두. */
export function applyPanelTitles(api: DockviewApi, titles: Readonly<Record<string, string>>) {
  const apply = (panel: {
    id: string;
    title: string | undefined;
    api: { setTitle(t: string): void };
  }) => {
    const title = titles[panel.id];
    if (title !== undefined && panel.title !== title) panel.api.setTitle(title);
  };
  api.panels.forEach(apply);
  return api.onDidAddPanel(apply);
}

/**
 * 닫힌 툴 윈도우를 숨김으로 기록한다(머리 주석). 닫을 때 있던 곳을 기억해 다시 열 때 그리로 넣는다.
 * dockview 는 옮기기 · `clear` 에서도 패널을 지웠다 다시 넣으므로, 흐름이 끝난 뒤에도 없을 때만 숨김으로 본다.
 */
export function trackHiddenPanels(api: DockviewApi) {
  const hidden = hiddenOf(api);
  const removed = api.onDidRemovePanel((panel) => {
    const known = spec(panel.id);
    if (!known) return;
    let place: Place;
    try {
      place = placeOf(api, panel.group.id);
    } catch {
      place = known.home;
    }
    queueMicrotask(() => {
      if (!api.getPanel(panel.id)) hidden.set(panel.id, place);
    });
  });
  const added = api.onDidAddPanel((panel) => hidden.delete(panel.id));
  return {
    dispose() {
      removed.dispose();
      added.dispose();
    },
  };
}

/** 툴 윈도우 바 · 열기 액션에서 고른 패널을 앞에 띄운다. 숨어 있으면 숨길 때 있던 곳(없으면 `home`)에 다시 연다. */
export function showPanel(api: DockviewApi, id: string) {
  const existing = api.getPanel(id);
  if (existing) {
    existing.api.setActive();
    expandDock(existing.group.api);
    return;
  }
  const panel = spec(id);
  if (!panel) return;
  const place = hiddenOf(api).get(id);
  hiddenOf(api).delete(id);
  addPanel(api, panel, false, place).api.setActive();
}

/**
 * 툴 윈도우 열기 액션(⌘1 등). IntelliJ 처럼, 이미 활성인 툴 윈도우면 그 dock 을 접어 감추고(상태는 그대로), 아니면 띄운다.
 * 가운데 grid 로 옮겨 둔 툴 윈도우는 접을 dock 이 없어 늘 띄우기만 한다.
 */
export function toggleToolWindow(api: DockviewApi, id: string) {
  const existing = api.getPanel(id);
  if (existing && api.activePanel?.id === id) {
    const group = existing.group.api;
    if (group.location.type === "edge" && !group.isCollapsed()) {
      group.collapse();
      return;
    }
  }
  showPanel(api, id);
}

/** 모든 툴 윈도우 dock 을 접는다(`toolWindow.hideAll`). 패널은 그대로 두므로 다시 열면 그 상태로 돌아온다. */
export function collapseAllDocks(api: DockviewApi) {
  for (const dock of DOCKS) {
    const group = api.getEdgeGroup(dock);
    if (group && !group.isCollapsed()) group.collapse();
  }
}
