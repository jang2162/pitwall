// 시험용: happy-dom 은 배치를 하지 않아 요소의 크기가 0 이고 scrollTo 가 scroll 이벤트를 내지 않는다. 트리의 가상화(TanStack
// Virtual)가 읽는 값만 준다 — 트리(`role="tree"`)의 보이는 높이 · 굴릴 수 있는 높이와, scrollTo 뒤의 scroll 이벤트.

import { vi } from "vitest";

/** `vi.restoreAllMocks()` 가 되돌린다. */
export function fakeTreeLayout(viewHeight: number) {
  const isTree = (el: HTMLElement) => el.getAttribute("role") === "tree";
  for (const prop of ["offsetHeight", "clientHeight"] as const)
    vi.spyOn(HTMLElement.prototype, prop, "get").mockImplementation(function (this: HTMLElement) {
      return isTree(this) ? viewHeight : 0;
    });
  // 굴릴 수 있는 끝은 안쪽 요소(행 전체 높이)의 높이다.
  vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(function (
    this: HTMLElement,
  ) {
    const inner = isTree(this) ? this.firstElementChild : null;
    return inner instanceof HTMLElement ? Number.parseFloat(inner.style.height) || 0 : 0;
  });
  vi.spyOn(HTMLElement.prototype, "scrollTo").mockImplementation(function (
    this: HTMLElement,
    options?: ScrollToOptions | number,
  ) {
    if (typeof options === "object" && options.top !== undefined) this.scrollTop = options.top;
    this.dispatchEvent(new Event("scroll"));
  });
}
