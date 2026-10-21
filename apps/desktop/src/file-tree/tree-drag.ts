// 파일 트리 안 드래그(M1-21, 10-ui §5.4): 행을 끌어 폴더(또는 파일 — 그 부모)에 놓으면 옮기고, ⌥ 를 누른 채 놓으면 복사한다.
//
// HTML5 드래그(draggable · dragover · drop)가 아니라 포인터 이벤트로 끈다. Finder 드롭을 받으려고 켠 Tauri 의 드래그 드롭 처리기가
// 늘 「처리했다」 를 돌려줘(tauri-runtime-wry) wry 가 WKWebView 의 원래 드래그 콜백을 부르지 않는다 — 웹뷰 안의 dragover · drop 이
// 오지 않는다. Windows 의 WebView2 도 Tauri 가 자기 드롭 대상을 등록해 같다. Finder 드롭은 그 처리기가 받는다(tree-drop.ts).
//
// - 누른 자리에서 DRAG_THRESHOLD 이상 움직여야 끌기가 시작된다. 그 전에 떼면 보통의 클릭이다. 끈 뒤의 클릭은 삼킨다.
// - 끄는 동안 놓을 폴더를 알린다(행 강조). 트리 밖이거나 끄는 항목 자신 · 그 안이면 놓을 곳이 없다.
// - 트리 밖(창 밖 포함)에서 떼거나, Esc · 포인터 취소 · 창이 포커스를 잃으면 아무것도 하지 않고 끝낸다.
// - ⌥ 는 포인터 · 키 이벤트의 `altKey` 로 본다. 수식키 상태는 IME 조합 중인 키(keyCode 229)에도 맞게 실리므로 조합 여부로 가리지
//   않는다. 놓을 때는 떼는 순간의 `altKey` 가 정한다.
// - 트리의 위 · 아래 가장자리(AUTO_SCROLL_EDGE) 가까이 또는 그 바깥에 있으면 굴린다. 가장자리에 깊이 들어갈수록 빠르다.

import { isComposingKey } from "../ui/ime";
import { isWithin } from "./tree-model";

/** 끌기가 시작되는 거리(CSS 픽셀). */
export const DRAG_THRESHOLD = 4;
/** 이 거리(CSS 픽셀) 안의 위 · 아래 가장자리에서 굴린다. */
export const AUTO_SCROLL_EDGE = 24;
/** 한 그림에 굴리는 최대 거리(CSS 픽셀). */
export const AUTO_SCROLL_MAX = 16;

/** 끄는 중의 상태. `dir` 은 지금 놓으면 들어갈 폴더(없으면 놓을 곳이 없다). */
export interface TreeDragState {
  dir: string | undefined;
  copy: boolean;
}

export interface TreeDragOptions {
  /** 굴리는 트리 요소(`role=tree`). */
  tree: HTMLElement;
  paths: readonly string[];
  /** 누른 자리(CSS 픽셀). */
  start: { x: number; y: number };
  pointerId: number;
  /** 이 요소 위에 놓으면 들어갈 폴더. 트리 밖이면 `undefined`. */
  folderAt(element: Element | null): string | undefined;
  /** 끌기가 시작됐거나 놓을 곳 · 복사 여부가 바뀌었다. 끝나면 `undefined`. */
  onChange(state: TreeDragState | undefined): void;
  onDrop(paths: readonly string[], dir: string, copy: boolean): void;
  /** 다음 그림 차례에 부른다(기본 requestAnimationFrame). 취소 함수를 돌려준다. */
  frame?(callback: () => void): () => void;
}

const nextFrame = (callback: () => void) => {
  const id = requestAnimationFrame(callback);
  return () => cancelAnimationFrame(id);
};

/**
 * 행을 누른 뒤부터 뗄 때까지 포인터를 따라간다. 돌려준 함수는 놓지 않고 끝낸다(트리가 사라질 때).
 */
export function trackTreeDrag(o: TreeDragOptions): () => void {
  const frame = o.frame ?? nextFrame;
  /** pending: 문턱 전. dragging: 끄는 중. cancelled: 취소했고 뗄 때를 기다린다(그 클릭을 삼키려고). */
  let phase: "pending" | "dragging" | "cancelled" | "done" = "pending";
  let state: TreeDragState = { dir: undefined, copy: false };
  let point = o.start;
  let stopScroll: (() => void) | undefined;

  const target = (element: Element | null) => {
    const dir = o.folderAt(element);
    return dir === undefined || o.paths.some((p) => isWithin(dir, p)) ? undefined : dir;
  };
  const show = (dir: string | undefined, copy: boolean) => {
    if (dir === state.dir && copy === state.copy) return;
    state = { dir, copy };
    o.onChange(state);
  };

  const scrollStep = (): number => {
    const rect = o.tree.getBoundingClientRect();
    if (rect.height === 0 || point.x < rect.left || point.x > rect.right) return 0;
    const speed = (depth: number) =>
      Math.min(
        AUTO_SCROLL_MAX,
        Math.max(1, Math.ceil((depth / AUTO_SCROLL_EDGE) * AUTO_SCROLL_MAX)),
      );
    if (point.y < rect.top + AUTO_SCROLL_EDGE) return -speed(rect.top + AUTO_SCROLL_EDGE - point.y);
    if (point.y > rect.bottom - AUTO_SCROLL_EDGE)
      return speed(point.y - (rect.bottom - AUTO_SCROLL_EDGE));
    return 0;
  };
  const scroll = () => {
    stopScroll = undefined;
    const step = scrollStep();
    if (phase !== "dragging" || step === 0) return;
    const before = o.tree.scrollTop;
    o.tree.scrollTop = before + step;
    // 행이 포인터 밑으로 움직였다 — 포인터는 그대로라 이벤트가 오지 않는다.
    if (o.tree.scrollTop !== before)
      show(target(document.elementFromPoint(point.x, point.y)), state.copy);
    stopScroll = frame(scroll);
  };

  const swallowClick = () => {
    const swallow = (e: MouseEvent) => {
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("click", swallow, { capture: true, once: true });
    setTimeout(() => window.removeEventListener("click", swallow, { capture: true }), 0);
  };

  const finish = () => {
    if (phase === "done") return;
    const shown = phase === "dragging";
    phase = "done";
    stopScroll?.();
    stopScroll = undefined;
    window.removeEventListener("pointermove", onMove, true);
    window.removeEventListener("pointerup", onUp, true);
    window.removeEventListener("pointercancel", onCancel, true);
    window.removeEventListener("keydown", onKey, true);
    window.removeEventListener("keyup", onKey, true);
    window.removeEventListener("blur", onCancel);
    if (shown) o.onChange(undefined);
  };
  /** 놓지 않고 끝낸다. 끄던 중이면 뗄 때의 클릭을 삼키려고 그때까지 기다린다. */
  const cancel = () => {
    if (phase !== "dragging") return finish();
    phase = "cancelled";
    stopScroll?.();
    stopScroll = undefined;
    o.onChange(undefined);
  };

  const onMove = (e: PointerEvent) => {
    if (e.pointerId !== o.pointerId || phase === "cancelled") return;
    point = { x: e.clientX, y: e.clientY };
    if (phase === "pending") {
      if (Math.hypot(point.x - o.start.x, point.y - o.start.y) < DRAG_THRESHOLD) return;
      phase = "dragging";
      state = { dir: target(e.target as Element | null), copy: e.altKey };
      o.onChange(state);
    } else {
      show(target(e.target as Element | null), e.altKey);
    }
    e.preventDefault();
    if (stopScroll === undefined && scrollStep() !== 0) stopScroll = frame(scroll);
  };
  const onUp = (e: PointerEvent) => {
    if (e.pointerId !== o.pointerId) return;
    if (phase === "pending") return finish();
    swallowClick();
    const dir = phase === "dragging" ? target(e.target as Element | null) : undefined;
    finish();
    if (dir !== undefined) o.onDrop(o.paths, dir, e.altKey);
  };
  const onCancel = () => cancel();
  const onKey = (e: KeyboardEvent) => {
    if (phase !== "dragging") return;
    if (e.type === "keydown" && e.key === "Escape" && !isComposingKey(e)) {
      e.preventDefault();
      e.stopPropagation();
      cancel();
      return;
    }
    show(state.dir, e.altKey);
  };

  window.addEventListener("pointermove", onMove, true);
  window.addEventListener("pointerup", onUp, true);
  window.addEventListener("pointercancel", onCancel, true);
  window.addEventListener("keydown", onKey, true);
  window.addEventListener("keyup", onKey, true);
  window.addEventListener("blur", onCancel);
  return finish;
}
