// 탐색기의 테이블 · 뷰를 SQL 콘솔로 끌어 이름을 넣는다(10-ui.md 「DB」 마우스 표 — `⌥` 면 `SELECT * FROM …`).
//
// HTML5 드래그가 아니라 포인터 이벤트로 끈다 — Tauri 의 드래그 드롭 처리기가 웹뷰 안의 dragover · drop 을 가져간다(파일 트리의
// src/file-tree/tree-drag.ts 머리 주석과 같은 까닭). 놓은 자리의 요소에서 CodeMirror 에디터(`EditorView.findFromDOM`)를 찾아 그
// 자리(캐럿 자리, 못 찾으면 커서)에 넣는다. 콘솔(M3-15)은 CodeMirror 문서라 따로 받는 자리가 없어도 된다 — 같은 까닭으로 고칠 수
// 있는 다른 에디터 탭에도 들어간다. 읽기 전용 에디터(`EditorState.readOnly`)에는 넣지 않는다.
//
// - 누른 자리에서 DRAG_THRESHOLD 이상 움직여야 끌기가 시작된다. 그 전에 떼면 보통의 클릭이다. 끈 뒤의 클릭은 삼킨다.
// - `⌥` 는 떼는 순간의 `altKey` 다(파일 트리의 복사와 같다).
// - 에디터가 아닌 곳에서 떼거나, Esc · 포인터 취소 · 창이 포커스를 잃으면 아무것도 하지 않고 끝낸다.

import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";

import { isComposingKey } from "../../ui/ime";

/** 끌기가 시작되는 거리(CSS 픽셀). */
export const DRAG_THRESHOLD = 4;

export interface ExplorerDragOptions {
  start: { x: number; y: number };
  pointerId: number;
  /** 놓을 글(`select` 는 떼는 순간 `⌥` 가 눌려 있었는가). */
  text(select: boolean): string;
  /** 끌기가 시작됐다(참) · 끝났다(거짓). 화면이 커서를 바꾼다. */
  onDragging(dragging: boolean): void;
}

/** 그 자리의 에디터(고칠 수 있는 것만). */
export function editorAt(element: Element | null): EditorView | undefined {
  const dom = element?.closest(".cm-editor");
  if (!(dom instanceof HTMLElement)) return undefined;
  const view = EditorView.findFromDOM(dom);
  if (!view || view.state.facet(EditorState.readOnly) || !view.state.facet(EditorView.editable))
    return undefined;
  return view;
}

/**
 * 화면 자리의 문서 위치. 브라우저의 캐럿 자리(`caretRangeFromPoint`)를 `posAtDOM` 으로 옮긴다 — `posAtCoords` 와 달리 배치를 다시
 * 재지 않는다. 에디터 밖이거나 못 찾으면 `undefined`.
 */
function posAt(view: EditorView, point: { x: number; y: number }): number | undefined {
  const range = document.caretRangeFromPoint?.(point.x, point.y);
  if (!range || !view.contentDOM.contains(range.startContainer)) return undefined;
  try {
    return view.posAtDOM(range.startContainer, range.startOffset);
  } catch {
    return undefined;
  }
}

/** 에디터의 그 자리(못 찾으면 커서)에 글을 넣고, 넣은 글 끝에 커서를 두고 에디터에 포커스를 준다. */
export function insertAt(view: EditorView, point: { x: number; y: number }, text: string) {
  const at = posAt(view, point) ?? view.state.selection.main.head;
  view.dispatch({
    changes: { from: at, insert: text },
    selection: { anchor: at + text.length },
    userEvent: "input.drop",
    scrollIntoView: true,
  });
  view.focus();
}

/** 포인터를 따라간다. 돌려준 함수가 멈춘다(언마운트). */
export function trackExplorerDrag(o: ExplorerDragOptions): () => void {
  let dragging = false;
  let done = false;

  const finish = () => {
    if (done) return;
    done = true;
    window.removeEventListener("pointermove", onMove, true);
    window.removeEventListener("pointerup", onUp, true);
    window.removeEventListener("pointercancel", onCancel, true);
    window.removeEventListener("keydown", onKey, true);
    window.removeEventListener("blur", onCancel);
    if (dragging) {
      o.onDragging(false);
      // 끈 뒤 떼는 자리에서 나는 클릭을 삼킨다.
      const swallow = (e: MouseEvent) => {
        e.stopPropagation();
        e.preventDefault();
      };
      window.addEventListener("click", swallow, { capture: true, once: true });
      setTimeout(() => window.removeEventListener("click", swallow, { capture: true }), 0);
    }
  };

  const onMove = (e: PointerEvent) => {
    if (e.pointerId !== o.pointerId || dragging) return;
    if (Math.hypot(e.clientX - o.start.x, e.clientY - o.start.y) < DRAG_THRESHOLD) return;
    dragging = true;
    o.onDragging(true);
  };

  const onUp = (e: PointerEvent) => {
    if (e.pointerId !== o.pointerId) return;
    if (dragging) {
      const point = { x: e.clientX, y: e.clientY };
      const view = editorAt(document.elementFromPoint(point.x, point.y));
      if (view) insertAt(view, point, o.text(e.altKey));
    }
    finish();
  };

  const onCancel = () => finish();
  const onKey = (e: KeyboardEvent) => {
    if (e.key !== "Escape" || isComposingKey(e)) return;
    e.preventDefault();
    e.stopPropagation();
    finish();
  };

  window.addEventListener("pointermove", onMove, true);
  window.addEventListener("pointerup", onUp, true);
  window.addEventListener("pointercancel", onCancel, true);
  window.addEventListener("keydown", onKey, true);
  window.addEventListener("blur", onCancel);
  return finish;
}
