// @vitest-environment happy-dom
// 트리 안 포인터 드래그(tree-drag.ts): 문턱, 놓을 폴더, ⌥ 복사, 취소(Esc · 트리 밖 · 포인터 취소 · 창 포커스), 자동 스크롤, 끈 뒤의 클릭.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  AUTO_SCROLL_EDGE,
  AUTO_SCROLL_MAX,
  DRAG_THRESHOLD,
  trackTreeDrag,
  type TreeDragOptions,
  type TreeDragState,
} from "./tree-drag";

let tree: HTMLElement;
let rows: { src: HTMLElement; a: HTMLElement; docs: HTMLElement; b: HTMLElement };
let outside: HTMLElement;
let changes: (TreeDragState | undefined)[];
let drops: { paths: readonly string[]; dir: string; copy: boolean }[];
let frames: (() => void)[];
let stop: () => void;

function row(path: string): HTMLElement {
  const el = document.createElement("div");
  el.setAttribute("role", "treeitem");
  el.setAttribute("data-path", path);
  tree.append(el);
  return el;
}

/** `/r/src/a.rs` 를 끈다. 폴더 행이면 그 폴더, 파일 행이면 그 부모, 트리의 빈 자리면 루트, 트리 밖이면 없다. */
function start(over: Partial<TreeDragOptions> = {}) {
  stop = trackTreeDrag({
    tree,
    paths: ["/r/src/a.rs"],
    start: { x: 10, y: 10 },
    pointerId: 1,
    folderAt: (el) => {
      if (!el || !tree.contains(el)) return undefined;
      const path = el.closest("[role=treeitem]")?.getAttribute("data-path");
      if (!path) return "/r";
      return path.endsWith(".rs") ? path.slice(0, path.lastIndexOf("/")) : path;
    },
    onChange: (s) => changes.push(s),
    onDrop: (paths, dir, copy) => drops.push({ paths, dir, copy }),
    frame: (cb) => {
      frames.push(cb);
      return () => frames.splice(frames.indexOf(cb), 1);
    },
    ...over,
  });
}

function pointer(el: Element, type: string, init: PointerEventInit = {}) {
  el.dispatchEvent(
    new PointerEvent(type, { bubbles: true, cancelable: true, pointerId: 1, ...init }),
  );
}
const move = (el: Element, x: number, y: number, init: PointerEventInit = {}) =>
  pointer(el, "pointermove", { clientX: x, clientY: y, ...init });
const up = (el: Element, init: PointerEventInit = {}) =>
  pointer(el, "pointerup", { clientX: 10, clientY: 60, ...init });
function key(type: "keydown" | "keyup", init: KeyboardEventInit & { keyCode?: number }) {
  const event = new KeyboardEvent(type, { bubbles: true, cancelable: true, ...init });
  if (init.keyCode !== undefined) Object.defineProperty(event, "keyCode", { value: init.keyCode });
  document.body.dispatchEvent(event);
  return event;
}

beforeEach(() => {
  tree = document.createElement("div");
  tree.setAttribute("role", "tree");
  outside = document.createElement("div");
  document.body.append(tree, outside);
  rows = {
    src: row("/r/src"),
    a: row("/r/src/a.rs"),
    docs: row("/r/docs"),
    b: row("/r/docs/b.rs"),
  };
  changes = [];
  drops = [];
  frames = [];
});

afterEach(() => {
  stop?.();
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

describe("trackTreeDrag", () => {
  it("starts only past the threshold; letting go before it is a plain click", () => {
    start();
    move(rows.docs, 10 + DRAG_THRESHOLD - 1, 10);
    expect(changes).toEqual([]);
    up(rows.docs);
    expect(drops).toEqual([]);
    expect(changes).toEqual([]);
    // 끝났다 — 다음 움직임은 받지 않는다.
    move(rows.docs, 10, 60);
    expect(changes).toEqual([]);
  });

  it("follows the folder under the pointer and moves there, or copies when ⌥ is down on letting go", () => {
    start();
    move(rows.docs, 10, 10 + DRAG_THRESHOLD);
    expect(changes.at(-1)).toEqual({ dir: "/r/docs", copy: false });
    // 파일 위면 그 부모.
    move(rows.b, 10, 40);
    expect(changes.at(-1)).toEqual({ dir: "/r/docs", copy: false });
    move(tree, 10, 50);
    expect(changes.at(-1)).toEqual({ dir: "/r", copy: false });
    up(rows.docs);
    expect(drops).toEqual([{ paths: ["/r/src/a.rs"], dir: "/r/docs", copy: false }]);
    expect(changes.at(-1)).toBeUndefined();

    changes = [];
    drops = [];
    start();
    move(rows.docs, 10, 40, { altKey: true });
    expect(changes.at(-1)).toEqual({ dir: "/r/docs", copy: true });
    // 떼는 순간의 ⌥ 가 정한다.
    up(rows.docs, { altKey: false });
    expect(drops).toEqual([{ paths: ["/r/src/a.rs"], dir: "/r/docs", copy: false }]);
    start();
    move(rows.docs, 10, 40);
    up(rows.docs, { altKey: true });
    expect(drops.at(-1)).toEqual({ paths: ["/r/src/a.rs"], dir: "/r/docs", copy: true });
  });

  it("reads ⌥ from key events even while the IME composes", () => {
    start();
    move(rows.docs, 10, 40);
    expect(changes.at(-1)).toEqual({ dir: "/r/docs", copy: false });
    // 한글 입력기 상태에서 ⌥ 를 누르면 조합 중인 키(keyCode 229)로 올 수 있다 — 수식키 상태는 맞다.
    key("keydown", { key: "Alt", altKey: true, isComposing: true, keyCode: 229 });
    expect(changes.at(-1)).toEqual({ dir: "/r/docs", copy: true });
    key("keyup", { key: "Alt", altKey: false, keyCode: 229 });
    expect(changes.at(-1)).toEqual({ dir: "/r/docs", copy: false });
  });

  it("has no target outside the tree or on the dragged item and what is inside it", () => {
    start({ paths: ["/r/src"] });
    move(rows.docs, 10, 40);
    expect(changes.at(-1)).toEqual({ dir: "/r/docs", copy: false });
    move(rows.src, 10, 40);
    expect(changes.at(-1)).toEqual({ dir: undefined, copy: false });
    move(rows.a, 10, 40);
    expect(changes.at(-1)).toEqual({ dir: undefined, copy: false });
    up(rows.a);
    expect(drops).toEqual([]);

    start();
    move(rows.docs, 10, 40);
    move(outside, 500, 40);
    expect(changes.at(-1)).toEqual({ dir: undefined, copy: false });
    up(outside);
    expect(drops).toEqual([]);
    // 창 밖에서 떼면 문서(창)가 받는다.
    start();
    move(rows.docs, 10, 40);
    document.dispatchEvent(
      new PointerEvent("pointerup", { bubbles: true, pointerId: 1, clientX: -5, clientY: -5 }),
    );
    expect(drops).toEqual([]);
    expect(changes.at(-1)).toBeUndefined();
  });

  it("cancels with Esc (not the IME's Esc), a pointer cancel or losing the window focus", () => {
    start();
    move(rows.docs, 10, 40);
    const composing = key("keydown", { key: "Escape", isComposing: true });
    expect(composing.defaultPrevented).toBe(false);
    expect(changes.at(-1)).toEqual({ dir: "/r/docs", copy: false });
    const esc = key("keydown", { key: "Escape" });
    expect(esc.defaultPrevented).toBe(true);
    expect(changes.at(-1)).toBeUndefined();
    // 취소 뒤의 움직임 · 떼기는 아무것도 하지 않는다.
    move(rows.src, 10, 40);
    up(rows.docs);
    expect(drops).toEqual([]);
    expect(changes.at(-1)).toBeUndefined();

    start();
    move(rows.docs, 10, 40);
    pointer(rows.docs, "pointercancel");
    expect(changes.at(-1)).toBeUndefined();
    up(rows.docs);
    expect(drops).toEqual([]);

    start();
    move(rows.docs, 10, 40);
    window.dispatchEvent(new Event("blur"));
    expect(changes.at(-1)).toBeUndefined();
    up(rows.docs);
    expect(drops).toEqual([]);
  });

  it("ignores other pointers and ends without dropping when disposed", () => {
    start();
    move(rows.docs, 10, 40, { pointerId: 2 });
    expect(changes).toEqual([]);
    move(rows.docs, 10, 40);
    stop();
    expect(changes.at(-1)).toBeUndefined();
    up(rows.docs);
    expect(drops).toEqual([]);
  });

  it("swallows the click that follows a drag, not the next one", async () => {
    const clicks = vi.fn();
    rows.docs.addEventListener("click", clicks);
    start();
    move(rows.docs, 10, 40);
    up(rows.docs);
    rows.docs.click();
    expect(clicks).not.toHaveBeenCalled();
    await new Promise((r) => setTimeout(r, 0));
    rows.docs.click();
    expect(clicks).toHaveBeenCalledTimes(1);

    // 문턱 전의 떼기는 클릭이다.
    start();
    move(rows.docs, 11, 10);
    up(rows.docs);
    rows.docs.click();
    expect(clicks).toHaveBeenCalledTimes(2);
  });

  it("scrolls near the top and bottom edges and judges the row that slid under the pointer", () => {
    vi.spyOn(tree, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 100, 200, 300));
    tree.scrollTop = 50;
    document.elementFromPoint = () => rows.src;
    start({ start: { x: 10, y: 200 } });
    move(rows.docs, 10, 250);
    expect(frames).toHaveLength(0);

    // 아래 가장자리 깊숙이: 한 그림에 최대만큼.
    move(rows.docs, 10, 400);
    expect(frames).toHaveLength(1);
    frames.shift()!();
    expect(tree.scrollTop).toBe(50 + AUTO_SCROLL_MAX);
    // 굴린 뒤 포인터 밑의 행으로 다시 판정한다.
    expect(changes.at(-1)).toEqual({ dir: "/r/src", copy: false });
    expect(frames).toHaveLength(1);

    // 위 가장자리 바로 안쪽: 느리게 위로.
    move(rows.docs, 10, 100 + AUTO_SCROLL_EDGE - 1);
    const before = tree.scrollTop;
    frames.shift()!();
    expect(tree.scrollTop).toBe(before - 1);

    // 가운데로 오면 멈춘다.
    move(rows.docs, 10, 250);
    frames.shift()!();
    expect(frames).toHaveLength(0);

    // 옆으로 벗어나면 굴리지 않는다.
    move(outside, 500, 400);
    expect(frames).toHaveLength(0);

    // 끝나면 굴리기도 멈춘다.
    move(rows.docs, 10, 400);
    expect(frames).toHaveLength(1);
    up(rows.docs);
    expect(frames).toHaveLength(0);
  });
});
