// 시험에서 에디터와 그 툴팁을 함께 찾는다 — 툴팁은 `.cm-editor` 밖의 툴팁 층에 있다(src/editor/tooltip-layer.ts).

import type { EditorView } from "@codemirror/view";

import { tooltipHostOf } from "../editor/tooltip-layer";

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
