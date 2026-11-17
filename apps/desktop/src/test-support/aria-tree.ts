// `role="tree"` 가 macOS 의 WebKit(앱의 웹뷰)에서 트리로 보이는가를 DOM 으로 본다. `none` 은 WebKit 이 같은 뜻으로 받지 않으므로 쓰지 않는다.
//
// WebKit 은 트리의 자손을 넓이 우선으로 걸으며 `treeitem` 이 아닌 요소가 `group` · `presentation` 이 아니면 그 트리를 「틀린 트리」 로
// 보고 generic 으로 내린다 — 그러면 그 안의 `treeitem` 도 트리 항목이 아니게 되어 접근성 트리에서 펼칠 수 없는 `group` 이 되고
// `aria-expanded` · 키보드가 보조 기술에 닿지 않는다(AccessibilityNodeObject::isValidTree). 역할 없는 `div` 로 감싼 것(가상 목록의
// 높이 상자 등)도 틀린 트리다 — WebKit 의 커밋 c5d00a47c2 가 역할 없는 `div` · `span` 을 건너뛰게 고쳤지만 그보다 앞의 시스템
// WebKit(macOS 27.2)은 그렇지 않다. `treeitem` 의 안쪽은 보지 않는다.

/** 트리를 틀린 트리로 만드는 자손(태그 · 역할 · 글). 비면 WebKit 이 트리로 내보인다. */
export function ariaTreeProblems(tree: Element): string[] {
  const problems: string[] = [];
  const queue = [...tree.children];
  while (queue.length > 0) {
    const el = queue.shift()!;
    const role = el.getAttribute("role");
    if (role === "treeitem") continue;
    if (role === "group" || role === "presentation") {
      queue.push(...el.children);
      continue;
    }
    problems.push(
      `<${el.tagName.toLowerCase()}${role ? ` role="${role}"` : ""}> ${(el.textContent ?? "").slice(0, 40)}`,
    );
  }
  return problems;
}
