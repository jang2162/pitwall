// 에디터 툴팁의 자리(15-m4-plan §3 「타이핑 지연 LSP 켬 − 끔」, 사람 결정 2026-11-13): 에디터의 툴팁(자동완성 · 호버 · 진단 · 시그니처 ·
// 코드 액션 · 이름 바꾸기 · git 거터)을 `.cm-editor` 안이 아니라 body 의 툴팁 층(`#pw-editor-tooltips`)에 그린다.
//
// - 까닭: 툴팁이 에디터 패널 안에 있으면 패널 경계에서 잘린다. 잘린 툴팁은 CodeMirror 툴팁 플러그인의 IntersectionObserver 가 키마다
//   비율 < 1 을 보고 50 ms 뒤 다시 재는 그리기를 걸어 다음 키의 프레임을 밀었다. 층에 두면 툴팁은 창 안에서 옆 툴 윈도우 · 분할
//   에디터 · 상태바 위로 겹쳐 뜬다(창 밖으로는 나가지 않는다 — CodeMirror 가 창 크기 안으로 맞춘다).
// - 겹침 순서: 층은 `z-index: TOOLTIP_LAYER_Z`(2000)다 — 패널 안의 `z-10` · `z-20`, dockview 의 탭(5 · 10)과 플로팅 그룹
//   (`999 + 2i`) 위. 창 안 모달(탭 전환기 · 탐색 팝업)과 blame 메뉴는 `OVERLAY_Z`(2100)로 층 위에 둔다. Radix 대화상자 · 메뉴는
//   z-index 없이 body 끝에 붙어 층 아래로 겹치므로, 모달이 열려 Radix 가 나머지에 `aria-hidden` 을 걸면 층을 숨긴다(styles.css 의
//   `#pw-editor-tooltips[aria-hidden="true"]`). `modal={false}` 인 Radix 나 Radix 가 아닌 body 포털이 생기면 층이 그 위로 온다 —
//   그런 것은 `OVERLAY_Z` 를 단다.
// - 글자 선택: body 의 `user-select: none`(styles.css)을 층에서 푼다 — 호버 · 진단 글을 전처럼 끌어 골라 복사한다.
// - 보임: 에디터가 숨으면(다른 탭 · 비교 화면 — dockview 는 숨은 탭의 DOM 을 떼어 낸다) 툴팁도 숨긴다(`setTooltipsShown`). 전에는
//   툴팁이 `view.dom` 안이라 함께 떨어졌다.
// - 키맵 문맥: 에디터마다 호스트(`.pw-editor-tooltip-host`) 하나를 층에 두고, 에디터를 감싼 `data-keymap-context` ·
//   `data-tool-window` 를 호스트에 옮겨 적는다(뷰 갱신마다 — 툴팁은 갱신으로만 열린다) — 팝업 안(이름 바꾸기 칸 · 코드 액션 단추 ·
//   링크)에 포커스가 있을 때의 키가 전처럼 그 에디터의 문맥으로 처리된다(src/keymap/context.ts 는 포커스에서 가장 가까운 것을 본다).
//   포커스가 팝업 안일 때 「그 에디터」 는 `editorOfElement` 로 찾는다.
// - `view.dom` 에 단 이벤트 처리는 툴팁 안의 이벤트를 받지 못한다. 그것에 기대는 곳은 `tooltipHostOf` 로 호스트에도 단다
//   (lsp/navigation.ts 의 호버 `mouseleave`).
// - 범위: 편집기 확장(`editor/extensions/base.ts`)을 쓰는 메인 에디터만이다. 따로 만든 EditorView(DB 콘솔 · diff · 병합 · DDL ·
//   찾기 미리보기)의 툴팁은 전처럼 그 뷰 안에 있다.

import type { Extension } from "@codemirror/state";
import { EditorView, tooltips, ViewPlugin } from "@codemirror/view";

export const TOOLTIP_LAYER_ID = "pw-editor-tooltips";
/** 패널 안의 겹침(`z-10` · `z-20`, dockview 탭 5 · 10, 플로팅 그룹 `999 + 2i`) 위, `OVERLAY_Z` 아래. */
export const TOOLTIP_LAYER_Z = 2000;
/** 툴팁 층 위에 올 창 안 겹침(탭 전환기 · 탐색 팝업 — `z-[2100]` — · blame 메뉴). */
export const OVERLAY_Z = 2100;
export const TOOLTIP_HOST_CLASS = "pw-editor-tooltip-host";

/** 옮겨 적는 속성 — 키맵의 포커스 층(src/keymap/context.ts)이 가장 가까운 것을 본다. */
const CARRIED = ["data-keymap-context", "data-tool-window"] as const;

const hosts = new WeakMap<EditorView, HTMLElement>();
/** 숨긴 에디터(`setTooltipsShown(view, false)`) — 다시 구성해 생긴 새 호스트도 따른다. */
const hidden = new WeakSet<EditorView>();
const views = new WeakMap<Element, EditorView>();

/** 문서의 툴팁 층. 없으면 `#root` 바로 뒤(없으면 body 끝)에 만든다 — Radix 의 포털보다 앞에 있어야 모달이 그 위에 그려진다. */
export function tooltipLayer(doc: Document = document): HTMLElement {
  const found = doc.getElementById(TOOLTIP_LAYER_ID);
  if (found) return found;
  const layer = doc.createElement("div");
  layer.id = TOOLTIP_LAYER_ID;
  layer.style.position = "relative";
  layer.style.zIndex = String(TOOLTIP_LAYER_Z);
  const root = doc.getElementById("root");
  if (root?.parentElement === doc.body) root.after(layer);
  else doc.body.append(layer);
  return layer;
}

/** 에디터를 감싼 문맥 속성을 호스트에 옮겨 적는다(같으면 건드리지 않는다). */
function carry(view: EditorView, host: HTMLElement) {
  for (const name of CARRIED) {
    const value = view.dom.closest(`[${name}]`)?.getAttribute(name) ?? null;
    if (host.getAttribute(name) === value) continue;
    if (value === null) host.removeAttribute(name);
    else host.setAttribute(name, value);
  }
}

/** 에디터의 툴팁을 툴팁 층에 그린다. 뷰마다(다시 구성할 때도) 새 호스트를 만들고, 플러그인이 내려가면 걷는다. */
export function editorTooltips(): Extension {
  const host = document.createElement("div");
  host.className = TOOLTIP_HOST_CLASS;
  return [
    tooltips({ parent: host }),
    ViewPlugin.define((view) => {
      tooltipLayer(view.dom.ownerDocument).append(host);
      hosts.set(view, host);
      views.set(host, view);
      host.hidden = hidden.has(view);
      carry(view, host);
      return {
        // 에디터가 DOM 에 붙거나 옮겨진 뒤의 첫 갱신에서 문맥을 다시 본다(툴팁은 갱신으로만 열린다).
        update: () => carry(view, host),
        destroy: () => {
          if (hosts.get(view) === host) hosts.delete(view);
          host.remove();
        },
      };
    }),
  ];
}

/** 에디터가 보이는지에 맞춰 그 툴팁을 보이거나 숨긴다(다른 탭 · 비교 화면으로 에디터가 숨을 때). */
export function setTooltipsShown(view: EditorView, shown: boolean) {
  if (shown) hidden.delete(view);
  else hidden.add(view);
  const host = hosts.get(view);
  if (host) host.hidden = !shown;
}

/** 에디터의 툴팁 호스트(툴팁 층에 둔 것). 이 확장이 없는 뷰면 `undefined`. */
export function tooltipHostOf(view: EditorView): HTMLElement | undefined {
  return hosts.get(view);
}

/** 요소가 든 에디터 — 글자 영역이든 그 에디터의 툴팁 안이든. */
export function editorOfElement(el: Element | null | undefined): EditorView | undefined {
  if (!el) return undefined;
  const dom = el.closest(".cm-editor");
  if (dom instanceof HTMLElement) return EditorView.findFromDOM(dom) ?? undefined;
  const host = el.closest(`.${TOOLTIP_HOST_CLASS}`);
  return host ? views.get(host) : undefined;
}
