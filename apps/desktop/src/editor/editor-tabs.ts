// 에디터 탭 조작(결정 M1-D1): 에디터 탭 하나 = dockview 패널 하나, 가운데 grid 의 그룹이 에디터 그룹이다. 분할 · 드래그 · 직렬화는
// dockview 가 하고, 여기는 탭의 뜻(어느 파일 · 미리보기 · 고정)과 그것을 지키는 조작만 둔다. 화면(탭 머리 · 뷰어)은 editor-tab.tsx ·
// editor-panel.tsx, 액션과 열기 경로는 editor-workspace.ts 다.
//
// - 탭의 파일은 패널 params 의 `path`(정규화한 실제 경로, 문서 모델의 키)다. 한 파일을 여러 그룹이 보일 수 있다(분할) — 패널 id 는
//   파일과 무관한 일련번호다.
// - 미리보기 탭(10-ui §5.4): 그룹마다 하나. 다른 파일을 미리보기로 열면 그 자리를 대신한다. 같은 파일을 고정으로 다시 열거나 고정
//   (pin)하면 보통 탭이 된다(`preview` 를 끈다). 이탤릭은 탭 머리가 그린다.
// - 고정(pin, 10-ui §5.2): 「다른 탭 닫기」 가 닫지 않는다. dockview 의 pinned tabs 는 코어에 없어(별도 모듈) params 로 둔다.
// - 최대화(탭 더블클릭): 툴 윈도우 dock 을 모두 접고, 다시 하면 접기 전에 펼쳐 있던 것만 편다.

import type { DockviewApi, DockviewGroupPanel, IDockviewPanel } from "dockview-react";

/** 에디터 탭의 내용 컴포넌트 이름(project-window.tsx 의 COMPONENTS 키). */
export const EDITOR_PANEL = "editorTab";
/** 에디터 탭의 탭 머리 이름(project-window.tsx 의 TAB_COMPONENTS 키). */
export const EDITOR_TAB_HEADER = "editorTab";

export interface EditorTabParams {
  /** 정규화한 실제 경로. */
  path: string;
  /** 미리보기 탭(이탤릭, 다음 미리보기가 대신한다). */
  preview?: boolean;
  /** 고정(pin). */
  pinned?: boolean;
}

export function isEditorPanel(panel: IDockviewPanel | undefined): panel is IDockviewPanel {
  return panel?.api.component === EDITOR_PANEL;
}

export function tabParams(panel: IDockviewPanel): EditorTabParams {
  return (panel.params ?? { path: "" }) as unknown as EditorTabParams;
}

export function editorPanels(api: DockviewApi): IDockviewPanel[] {
  return api.panels.filter(isEditorPanel);
}

/** 경로의 마지막 조각(탭 이름). */
export function baseName(path: string): string {
  const trimmed = path.replace(/[/\\]+$/, "");
  return trimmed.slice(Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\")) + 1) || path;
}

function isGrid(group: DockviewGroupPanel): boolean {
  return group.api.location.type === "grid";
}

/**
 * 새 탭이 들어갈 그룹: 활성 그룹이 grid 면 그것, 아니면 마지막으로 쓴 에디터 탭(`recent` 앞쪽)의 grid 그룹, 그것도 없으면 첫 grid
 * 그룹. grid 가 비었으면(빈 영역 표시) 그룹을 만든다. 툴 윈도우 dock(edge group)에는 넣지 않는다.
 */
export function targetGroup(api: DockviewApi, recent: readonly string[] = []): DockviewGroupPanel {
  const active = api.activeGroup;
  if (active && isGrid(active)) return active;
  for (const id of recent) {
    const panel = api.getPanel(id);
    if (panel && isGrid(panel.group)) return panel.group;
  }
  return api.groups.find(isGrid) ?? api.addGroup();
}

let serial = 0;

function nextPanelId(api: DockviewApi): string {
  let id: string;
  do id = `editor-${++serial}`;
  while (api.getPanel(id));
  return id;
}

function setParams(panel: IDockviewPanel, change: Partial<EditorTabParams>) {
  panel.api.updateParameters({ ...tabParams(panel), ...change });
}

/** 미리보기를 보통 탭으로 바꾼다. */
export function keepTab(panel: IDockviewPanel) {
  if (tabParams(panel).preview) setParams(panel, { preview: false });
}

/** 그 파일을 보이는 탭. `group` 의 것을 먼저, 없으면 `recent` 순서(최근에 쓴 것 먼저), 그다음 아무 것. */
export function findTab(
  api: DockviewApi,
  path: string,
  { group, recent = [] }: { group?: DockviewGroupPanel; recent?: readonly string[] } = {},
): IDockviewPanel | undefined {
  const tabs = editorPanels(api).filter((p) => tabParams(p).path === path);
  if (tabs.length === 0) return undefined;
  const rank = (p: IDockviewPanel) => {
    if (group && p.group.id === group.id) return -1;
    const i = recent.indexOf(p.id);
    return i < 0 ? recent.length : i;
  };
  return tabs.sort((a, b) => rank(a) - rank(b))[0];
}

/**
 * 파일을 탭으로 연다. 이미 열려 있으면 새 탭 없이 그 탭을 활성으로(고정으로 열면 미리보기를 보통 탭으로) 한다. 아니면 대상
 * 그룹(`targetGroup`)에 새 탭을 연다 — 미리보기로 열면 그 그룹의 미리보기 탭 자리를 대신한다.
 */
export function openTab(
  api: DockviewApi,
  {
    path,
    preview = false,
    recent = [],
  }: { path: string; preview?: boolean; recent?: readonly string[] },
): { panel: IDockviewPanel; created: boolean } {
  const group = targetGroup(api, recent);
  const existing = findTab(api, path, { group, recent });
  if (existing) {
    if (!preview) keepTab(existing);
    existing.api.setActive();
    return { panel: existing, created: false };
  }
  const replaced = preview
    ? group.panels.find((p) => isEditorPanel(p) && tabParams(p).preview && !tabParams(p).pinned)
    : undefined;
  const index = replaced ? group.panels.indexOf(replaced) : undefined;
  replaced?.api.close();
  // 미리보기 자리를 닫아 그룹이 비면 dockview 가 그룹을 지운다 — 그때는 대상 그룹을 다시 고른다.
  const into = api.getGroup(group.id) ? group : targetGroup(api, recent);
  const params: EditorTabParams = { path, preview };
  const panel = api.addPanel({
    id: nextPanelId(api),
    component: EDITOR_PANEL,
    tabComponent: EDITOR_TAB_HEADER,
    title: baseName(path),
    params: params as unknown as Record<string, unknown>,
    position: { referenceGroup: into.id, index },
  });
  return { panel, created: true };
}

/** 같은 그룹의 다른 탭을 닫는다(⌘⌥W). 고정한 탭은 남긴다. */
export function closeOtherTabs(panel: IDockviewPanel) {
  // 닫는 동안 그룹의 탭 목록이 바뀌므로 먼저 고른다.
  const others = panel.group.panels.filter(
    (other) => other.id !== panel.id && isEditorPanel(other) && !tabParams(other).pinned,
  );
  for (const other of others) other.api.close();
}

/** 같은 파일을 오른쪽 새 그룹에 연다(⌘\). 새 탭이 활성이 된다. */
export function splitRight(api: DockviewApi, panel: IDockviewPanel): IDockviewPanel {
  const { path } = tabParams(panel);
  const params: EditorTabParams = { path };
  return api.addPanel({
    id: nextPanelId(api),
    component: EDITOR_PANEL,
    tabComponent: EDITOR_TAB_HEADER,
    title: baseName(path),
    params: params as unknown as Record<string, unknown>,
    position: { referencePanel: panel.id, direction: "right" },
  });
}

export function togglePinned(panel: IDockviewPanel) {
  const pinned = !tabParams(panel).pinned;
  setParams(panel, pinned ? { pinned, preview: false } : { pinned });
}

const DOCKS = ["left", "bottom", "right"] as const;
// 최대화로 접은 dock. api 마다 하나. 비어 있으면 최대화 상태가 아니다.
const maximizedByApi = new WeakMap<DockviewApi, (typeof DOCKS)[number][]>();

/** 에디터 최대화 토글(탭 더블클릭). 펼친 dock 이 있으면 모두 접고 기억한다. 없으면 기억한 것을 다시 편다. */
export function toggleEditorMaximized(api: DockviewApi) {
  const open = DOCKS.filter((d) => {
    const edge = api.getEdgeGroup(d);
    const panels = edge ? (api.getGroup(edge.id)?.panels.length ?? 0) : 0;
    return edge !== undefined && panels > 0 && !edge.isCollapsed();
  });
  if (open.length > 0) {
    for (const d of open) api.getEdgeGroup(d)?.collapse();
    maximizedByApi.set(api, open);
    return;
  }
  for (const d of maximizedByApi.get(api) ?? []) api.getEdgeGroup(d)?.expand();
  maximizedByApi.delete(api);
}

/**
 * 에디터 탭을 마지막으로 쓴 순서(앞이 최근)로 기억한다. ⌃Tab 전환기 · 새 탭의 그룹 · 「활성 탭」(외부 도구 문맥)이 쓴다. 닫힌 탭은
 * 빠진다.
 */
export function trackRecentTabs(api: DockviewApi) {
  const order: string[] = editorPanels(api).map((p) => p.id);
  const touch = (panel: IDockviewPanel | undefined) => {
    if (!isEditorPanel(panel)) return;
    const i = order.indexOf(panel.id);
    if (i >= 0) order.splice(i, 1);
    order.unshift(panel.id);
  };
  touch(api.activePanel);
  const activated = api.onDidActivePanelChange(({ panel }) => touch(panel));
  const removed = api.onDidRemovePanel((panel) => {
    // 옮기기(그룹 사이 드래그)는 지웠다 다시 넣는다 — 흐름이 끝난 뒤에도 없을 때만 뺀다.
    queueMicrotask(() => {
      if (api.getPanel(panel.id)) return;
      const i = order.indexOf(panel.id);
      if (i >= 0) order.splice(i, 1);
    });
  });
  return {
    /** 최근 순서의 패널 id. 아직 있는 것만. */
    order: (): string[] => order.filter((id) => api.getPanel(id)),
    dispose() {
      activated.dispose();
      removed.dispose();
    },
  };
}

export type RecentTabs = ReturnType<typeof trackRecentTabs>;
