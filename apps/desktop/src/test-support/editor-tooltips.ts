// 시험에서 에디터와 그 툴팁을 함께 찾는다 — 툴팁은 `.cm-editor` 밖의 툴팁 층에 있다(src/editor/tooltip-layer.ts).

import { StateEffect } from "@codemirror/state";
import { type EditorView, showTooltip, type Tooltip } from "@codemirror/view";
import { expect } from "vitest";

import { TOOLTIP_LAYER_ID, type TooltipTabSource, tooltipHostOf } from "../editor/tooltip-layer";

/** `view.dom` 과 그 에디터의 툴팁 호스트를 차례로 찾는 `querySelector` · `querySelectorAll`. */
export function withTooltips(view: EditorView) {
  const roots = (): ParentNode[] => {
    const host = tooltipHostOf(view);
    return host ? [view.dom, host] : [view.dom];
  };
  return {
    querySelector<E extends Element = HTMLElement>(selector: string): E | null {
      for (const root of roots()) {
        const found = root.querySelector<E>(selector);
        if (found) return found;
      }
      return null;
    },
    querySelectorAll<E extends Element = HTMLElement>(selector: string): E[] {
      return roots().flatMap((root) => [...root.querySelectorAll<E>(selector)]);
    },
    contains(node: Node | null): boolean {
      return roots().some((root) => (root as Node).contains(node));
    },
  };
}

/** 뷰에 시험 툴팁 하나를 띄우고 그 DOM 을 돌려준다(따로 만든 EditorView 의 툴팁 층 시험 — 16-m5-plan M5-21). */
export function showTestTooltip(view: EditorView): HTMLElement {
  const dom = document.createElement("div");
  dom.className = "pw-test-tip";
  const tip: Tooltip = { pos: 0, create: () => ({ dom }) };
  view.dispatch({ effects: StateEffect.appendConfig.of(showTooltip.of(tip)) });
  return dom;
}

/**
 * 툴팁 `tip` 이 툴팁 층의 그 뷰 호스트에 그려지고 뷰를 둔 패널(`panel`) 안에는 없다 — 패널 경계에 잘리지 않는다. 호스트를
 * 돌려준다.
 */
export function expectOnTooltipLayer(view: EditorView, tip: HTMLElement, panel: Element) {
  const host = tooltipHostOf(view);
  expect(host, "the view has a host in the tooltip layer").toBeDefined();
  expect(host!.contains(tip)).toBe(true);
  expect(document.getElementById(TOOLTIP_LAYER_ID)!.contains(host!)).toBe(true);
  expect(view.dom.contains(tip)).toBe(false);
  expect(panel.contains(tip)).toBe(false);
  return host!;
}

/** 시험의 탭(dockview 패널 api 의 보임 몫) — 탭 밖에 띄운 뷰에 `TooltipTab` 으로 준다. */
export function testTab(visible = true) {
  const listeners = new Set<(event: { isVisible: boolean }) => void>();
  let isVisible = visible;
  const tab = {
    get isVisible() {
      return isVisible;
    },
    onDidVisibilityChange(listener: (event: { isVisible: boolean }) => void) {
      listeners.add(listener);
      return { dispose: () => void listeners.delete(listener) };
    },
    setVisible(next: boolean) {
      isVisible = next;
      for (const l of listeners) l({ isVisible: next });
    },
  } satisfies TooltipTabSource & { setVisible(next: boolean): void };
  return tab;
}
