// 키맵 `when` 의 컨텍스트 층(packages/keymap/src/when.ts `ContextLayers`)을 DOM 의 포커스에서 만든다.
//
// - 포커스된 컴포넌트: 포커스가 든 요소에서 가장 가까운 `data-keymap-context="키 키 …"`. 입력 칸이면 `inputFocus`.
// - 툴 윈도우: 가장 가까운 `data-tool-window="<id>"` → `toolWindowFocus`, `activeToolWindow`.
// - 전역: `popupOpen`, `isMac` · `isWindows` · `isLinux`, 창이 내는 상태 키(`provideContext` — 예: `debugging`, 디버그 세션이 돈다,
//   src/debug/debug-feature.tsx).
// 컴포넌트가 자기 컨텍스트 키를 내려면 자기 뿌리 요소에 `data-keymap-context` 를 단다.

import type { ContextLayers, ContextValue, Platform } from "@pitwall/keymap";

type Layer = Record<string, ContextValue>;

// 창이 내는 전역 상태 키(포커스와 상관없는 것). 창(웹뷰)마다 모듈이 하나라 창마다 따로다.
const provided = new Map<string, () => ContextValue>();

/** 전역 층에 상태 키 `key` 를 낸다(키를 볼 때마다 `get` 을 부른다). 해제 함수 — 같은 키를 다시 내면 앞의 것을 덮는다. */
export function provideContext(key: string, get: () => ContextValue): () => void {
  provided.set(key, get);
  return () => {
    if (provided.get(key) === get) provided.delete(key);
  };
}

function isTextInput(el: Element): boolean {
  return (
    el instanceof HTMLTextAreaElement ||
    (el instanceof HTMLInputElement && !["button", "checkbox", "radio"].includes(el.type)) ||
    (el instanceof HTMLElement && el.isContentEditable)
  );
}

export function contextLayers(
  active: Element | null,
  global: { popupOpen: boolean; platform: Platform },
): ContextLayers {
  const focus: Layer = {};
  const toolWindow: Layer = {};
  if (active) {
    if (isTextInput(active)) focus.inputFocus = true;
    const keys = active.closest("[data-keymap-context]")?.getAttribute("data-keymap-context");
    for (const key of keys?.split(/\s+/).filter(Boolean) ?? []) focus[key] = true;
    const tool = active.closest("[data-tool-window]")?.getAttribute("data-tool-window");
    if (tool) {
      toolWindow.toolWindowFocus = true;
      toolWindow.activeToolWindow = tool;
    }
  }
  const state: Layer = {};
  for (const [key, get] of provided) state[key] = get();
  return [
    focus,
    toolWindow,
    {
      ...state,
      popupOpen: global.popupOpen,
      isMac: global.platform === "mac",
      isWindows: global.platform === "win",
      isLinux: global.platform === "linux",
    },
  ];
}
