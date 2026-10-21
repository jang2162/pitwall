// 프로젝트 창의 도킹 레이아웃(docs/design/10-ui.md §2.4): 왼쪽 · 오른쪽 · 하단 툴 윈도우와 가운데 에디터 영역.
// 툴 윈도우는 dockview 의 edge group 에, 에디터 탭은 가운데 grid 의 그룹(에디터 그룹)에 둔다(결정 M1-D1, src/editor/editor-tabs.ts).
// 툴 윈도우는 아직 대부분 자리만 잡은 빈 패널이다.
//
// 저장본은 `{ version, layout: dockview.toJSON() }` 이다. 읽을 수 없거나(JSON · 버전 · dockview 검사) 이 앱이 모르는
// 패널이 들어 있으면 버리고 기본 레이아웃으로 연다. 모르는 패널을 그대로 열면 dockview 는 오류 없이 빈 탭을 만든다.
//
// 저장소는 프로젝트 상태(layout/project-state.ts, `$DATA/projects/<id>/state.json`)의 레이아웃 칸이다 — 프로젝트마다 따로다.
// 에디터 탭은 저장본에서 뺀다(`stripPanels`). 탭은 워크트리마다 따로라(활성 워크트리의 탭 묶음, layout/worktree-session.ts) 레이아웃이
// 되살리면 다른 워크트리의 탭이 열린다. 탭을 빼서 빈 grid 그룹은 복원한 뒤 지운다. 에디터 탭이 하나도 없으면 가운데는 빈 영역 표시(dockview watermark, project-window.tsx)다. 예전 저장본의
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

// 저장소 안의 레이아웃 칸 이름. 예전에는 웹뷰 localStorage 의 이 키를 모든 프로젝트 창이 함께 썼다 — 프로젝트 상태가 처음 한 번
// 옮기고 지운다(project-state.ts).
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
 * 레이아웃이 바뀔 때마다 저장소에 넣는다. 크기 조절처럼 잇달아 오는 변경은 한 프레임(`frameMs`) 안의 것만 모아 한 번 넣는다 —
 * 저장소(프로젝트 상태)는 메모리에 넣고 디스크 쓰기는 따로 모아 한다(project-state.ts). 창을 닫으면(pagehide) 기다리던 것을 바로
 * 넣는다. ⚠ 앱 종료(메뉴 Quit · ⌘Q)에는 pagehide · beforeunload · unload · visibilitychange 가 오지 않는다(WKWebView 에서 잰
 * 것). 종료는 창마다 닫기 요청을 지나므로(src-tauri/src/menu.rs 의 `QUIT`) 프로젝트 창의 닫기 확인(project-window.tsx)이 레이아웃
 * 칸까지 마저 쓴다 — 그때 이 한 프레임 타이머가 아직이면 그 프레임의 변경은 빠진다. 강제 종료 · 크래시는 지켜지지 않는다.
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
  if (spec(id)) lastByApi.set(api, id);
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

// 마지막으로 쓴 툴 윈도우. dockview api 마다 하나(`toolWindow.last`, F12). 사람이 고른 것만 센다 — 포커스가 그 안에 들었거나
// 툴 윈도우 바 · 열기 액션으로 띄운 것(`showPanel`). 활성 패널 바뀜은 세지 않는다: 툴 윈도우를 숨기면 dockview 가 같은 dock 의
// 다른 패널을 활성으로 만드는데, 그때 F12 는 숨긴 것을 다시 열어야 한다(IntelliJ 와 같다).
const lastByApi = new WeakMap<DockviewApi, string>();

/** 포커스가 툴 윈도우 안으로 들어올 때마다 기억한다(「마지막 툴 윈도우로」). 해제 함수가 든 객체를 돌려준다. */
export function trackLastToolWindow(api: DockviewApi, doc: Document = document) {
  const onFocus = (e: FocusEvent) => {
    const id = (e.target as Element | null)
      ?.closest?.("[data-tool-window]")
      ?.getAttribute("data-tool-window");
    if (id && spec(id) && api.getPanel(id)) lastByApi.set(api, id);
  };
  doc.addEventListener("focusin", onFocus);
  return { dispose: () => doc.removeEventListener("focusin", onFocus) };
}

/** 마지막으로 쓴 툴 윈도우. 없으면 프로젝트 툴 윈도우. */
export function lastToolWindow(api: DockviewApi): string {
  return lastByApi.get(api) ?? PANELS[0]!.id;
}

/** 툴 윈도우 크기 조절 한 번의 폭(px). */
export const RESIZE_STEP = 20;
/** 크기 조절로 줄일 수 있는 가장 작은 크기(px). 더 줄이려면 접는다(숨기기). */
export const RESIZE_MIN = 60;

export type Arrow = "left" | "right" | "up" | "down";

/**
 * 툴 윈도우가 든 dock 을 화살표 쪽으로 늘리거나 줄인다(⌃⌥⇧←→↑↓, 10-ui 「툴 윈도우 · 창」). 화살표가 dock 의 경계를 미는
 * 쪽이면 늘고 반대면 준다 — 왼쪽 dock 은 → 가 늘리고, 오른쪽 dock 은 ← 가, 하단 dock 은 ↑ 가 늘린다. dock 과 엇갈린 화살표
 * (왼쪽 dock 의 ↑ 등)와 가운데로 옮긴 툴 윈도우는 아무것도 하지 않는다. 바꿨으면 `true`.
 */
export function resizeToolWindow(api: DockviewApi, id: string, arrow: Arrow): boolean {
  const panel = api.getPanel(id);
  if (!panel) return false;
  const group = panel.group.api;
  if (group.location.type !== "edge" || group.isCollapsed()) return false;
  const dock = placeOf(api, panel.group.id);
  const grow: Partial<Record<Arrow, number>> =
    dock === "left"
      ? { right: 1, left: -1 }
      : dock === "right"
        ? { left: 1, right: -1 }
        : dock === "bottom"
          ? { up: 1, down: -1 }
          : {};
  const sign = grow[arrow];
  if (sign === undefined) return false;
  if (dock === "bottom")
    group.setSize({ height: Math.max(RESIZE_MIN, group.height + sign * RESIZE_STEP) });
  else group.setSize({ width: Math.max(RESIZE_MIN, group.width + sign * RESIZE_STEP) });
  return true;
}

/** 모든 툴 윈도우 dock 을 접는다(`toolWindow.hideAll`). 패널은 그대로 두므로 다시 열면 그 상태로 돌아온다. */
export function collapseAllDocks(api: DockviewApi) {
  for (const dock of DOCKS) {
    const group = api.getEdgeGroup(dock);
    if (group && !group.isCollapsed()) group.collapse();
  }
}
