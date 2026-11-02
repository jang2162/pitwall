// LSP 팝업 바탕(15-m4-plan M4-07 ④): 호버 · 진단 · 시그니처 · 코드 액션 · 위치 목록이 함께 쓰는 에디터 팝업. 기능 행은 이 모듈을
// import 해 `openPopup` 으로 띄우고, 팝업 아래 액션 줄(`actions` 칸)에는 `popupActions` facet 으로 액션을 끼운다 — 진단 호버의
// 「빠른 수정」 은 M4-12 가 붙인다.
//
// - 그리는 자리: CodeMirror 툴팁(`showTooltip`)이라 `view.dom` 안에 그려진다. 위치 잡기 · 창 가장자리에서 위아래 뒤집기 · 가로로
//   창 안에 붙이기는 CodeMirror 의 툴팁이 한다(공간이 모자란 쪽을 피해 고른다). 한 뷰에 팝업은 하나다 — 새로 띄우면 앞 것을 바꾼다.
// - 닫힘: 편집(문서가 바뀜) · 스크롤 · `Esc`. `Esc` 는 CodeMirror 키맵(가장 높은 우선순위)이 받아 처리했다고 답하므로 keydown 이
//   `preventDefault` 된다 — 창 키맵의 `Esc` 「커서 하나로」(`yieldsToHandled`, M4-14)가 그 눌림에서 돌지 않아 멀티 커서가 남는다
//   (10-ui.md §4.4 「이미 처리된 키」). 팝업이 `view.dom` 밖(포털)에 그려지는 기능이 생기면 그 기능은 `useKeymapPopup` 으로 팝업이
//   열렸다고 알려야 Esc 가 팝업 몫이 된다(M4-14 ⓖ). 한글 조합 중의 Esc 는 CodeMirror 가 키맵에 넘기지 않아 조합만 취소한다.
// - 본문: Markdown 은 M1 의 안전한 렌더(src/editor/markdown.ts — raw HTML 끔)를 쓰고, 링크는 누름을 가로채 `http(s)` · `mailto` 만
//   외부 브라우저로 연다(`LspServices.openExternal` — 그 밖의 스킴 · 상대 경로는 열지 않는다). 코드 블록은 구문 강조 서비스로
//   칠한다(`LspServices.highlight` — 코드 블록의 언어, 없으면 문서의 언어). 서버가 준 글은 문자열로만 넣는다.
// - 색: 테마 토큰 `popup.*`(src/theme/tokens.ts).

import { Facet, Prec, StateEffect, StateField } from "@codemirror/state";
import { EditorView, keymap, showTooltip, type Tooltip, ViewPlugin } from "@codemirror/view";

import { editorContext } from "../editor/extensions/context";
import { tokenClass } from "../editor/extensions/highlight";
import { classifyLink, renderMarkdown } from "../editor/markdown";
import type { LspServices } from "./lsp-sessions";

/** 팝업의 종류. 액션 칸이 이것으로 고른다. */
export type PopupKind = "hover" | "diagnostic" | "signature" | "codeAction" | "locations";

export type PopupBody =
  /** 서버가 준 Markdown(`MarkupContent` · `MarkedString`). `language` 는 언어 없는 코드 블록의 언어(Linguist 이름). */
  | { kind: "markdown"; markdown: string; language?: string }
  /** 그대로의 글. */
  | { kind: "text"; text: string }
  /** 기능이 직접 그리는 본문(위치 목록 · 시그니처 등). */
  | { kind: "dom"; render(parent: HTMLElement, view: EditorView): void };

export interface PopupSpec {
  readonly kind: PopupKind;
  /** 붙는 자리(문서 위치). */
  readonly pos: number;
  readonly end?: number;
  /** 위에 먼저 놓는다(자리가 없으면 CodeMirror 가 뒤집는다). */
  readonly above?: boolean;
  readonly body: PopupBody;
  /** 이 팝업의 값(진단 등) — 액션 칸이 읽는다. */
  readonly data?: unknown;
}

export interface PopupAction {
  /** 시험 · 로그. */
  readonly id: string;
  readonly label: string;
  run(view: EditorView): void;
}

/** 팝업 아래 액션 줄(`actions` 칸)에 액션을 끼우는 것. `kinds` 의 팝업에서 `actions` 를 부른다. */
export interface PopupActionSource {
  readonly kinds: readonly PopupKind[];
  actions(popup: PopupSpec, view: EditorView): readonly PopupAction[];
}

/** 액션 칸. 기능 행이 자기 LSP 기능(`LSP_FEATURES`)의 확장에 `popupActions.of(…)` 를 넣는다. */
export const popupActions = Facet.define<PopupActionSource>();

/** 팝업을 띄운다(앞 것은 바뀐다). `null` 이면 닫는다. */
export const setPopup = StateEffect.define<PopupSpec | null>();

/** 지금 팝업. 편집하면 닫힌다. */
export const popupField = StateField.define<PopupSpec | null>({
  create: () => null,
  update(value, tr) {
    let next = value;
    for (const e of tr.effects) if (e.is(setPopup)) next = e.value;
    if (next !== null && tr.docChanged && next === value) return null;
    return next;
  },
  provide: (f) =>
    showTooltip.from(f, (spec): Tooltip | null =>
      spec === null
        ? null
        : {
            pos: spec.pos,
            ...(spec.end !== undefined && { end: spec.end }),
            above: spec.above ?? false,
            arrow: false,
            create: (view) => ({ dom: popupDom(view, spec) }),
          },
    ),
});

export function openPopup(view: EditorView, spec: PopupSpec) {
  view.dispatch({ effects: setPopup.of(spec) });
}

export function closePopup(view: EditorView) {
  if (view.state.field(popupField, false)) view.dispatch({ effects: setPopup.of(null) });
}

/** 지금 팝업(없거나 바탕이 없는 뷰면 `null`). */
export function currentPopup(view: EditorView): PopupSpec | null {
  return view.state.field(popupField, false) ?? null;
}

/** 이 창의 것(링크 · 코드 강조). 없으면 링크를 누를 수 없고 코드를 칠하지 않는다. */
function servicesOf(view: EditorView): LspServices {
  return view.state.facet(editorContext)?.lsp?.services ?? {};
}

const CODE_LANGUAGES: Record<string, string> = {
  rs: "Rust",
  ts: "TypeScript",
  typescriptreact: "TSX",
  tsx: "TSX",
  js: "JavaScript",
  javascriptreact: "JavaScript",
  jsx: "JavaScript",
  py: "Python",
  kt: "Kotlin",
  sh: "Shell",
  bash: "Shell",
};

/** 코드 블록의 언어 표지(`language-rust`) → Linguist 이름. 대소문자는 서비스가 가리지 않는다. */
export function codeLanguage(info: string | undefined, fallback: string | undefined) {
  const name = info?.trim().split(/\s+/)[0];
  if (!name) return fallback;
  return CODE_LANGUAGES[name.toLowerCase()] ?? name;
}

/** 코드 블록을 구문 강조 서비스로 칠한다(답이 오면 글자를 토큰 클래스의 조각으로 바꾼다). */
function highlightBlocks(root: HTMLElement, services: LspServices, fallback: string | undefined) {
  const { highlight } = services;
  if (!highlight) return;
  for (const code of root.querySelectorAll<HTMLElement>("pre > code")) {
    const info = /(?:^|\s)language-(\S+)/.exec(code.className)?.[1];
    const language = codeLanguage(info, fallback);
    const text = code.textContent ?? "";
    if (!language || text === "") continue;
    highlight(language, text).then(
      (spans) => {
        if (!code.isConnected || code.textContent !== text || spans.length === 0) return;
        const parts = document.createDocumentFragment();
        let at = 0;
        for (const span of spans) {
          if (span.from < at || span.to > text.length || span.from >= span.to) continue;
          if (span.from > at) parts.append(text.slice(at, span.from));
          const piece = document.createElement("span");
          piece.className = tokenClass(span.token);
          piece.textContent = text.slice(span.from, span.to);
          parts.append(piece);
          at = span.to;
        }
        if (at < text.length) parts.append(text.slice(at));
        code.replaceChildren(parts);
      },
      () => {},
    );
  }
}

function popupDom(view: EditorView, spec: PopupSpec): HTMLElement {
  const services = servicesOf(view);
  const dom = document.createElement("div");
  dom.className = "pw-lsp-popup";
  dom.dataset.popupKind = spec.kind;
  dom.setAttribute("role", "tooltip");
  const body = document.createElement("div");
  body.className = "pw-lsp-popup-body";
  const { body: content } = spec;
  if (content.kind === "markdown") {
    body.classList.add("pw-lsp-popup-markdown");
    // raw HTML 을 끈 렌더라 서버가 보낸 태그는 글자로 보인다(markdown.ts).
    body.innerHTML = renderMarkdown(content.markdown);
    const path = view.state.facet(editorContext)?.path ?? "";
    body.addEventListener("click", (e) => {
      const link = (e.target as Element | null)?.closest?.("a");
      if (!link) return;
      e.preventDefault();
      const target = classifyLink(link.getAttribute("href") ?? "", path, undefined);
      if (target.kind === "external") services.openExternal?.(target.url);
    });
    highlightBlocks(
      body,
      services,
      content.language ?? view.state.facet(editorContext)?.lsp?.language,
    );
  } else if (content.kind === "text") {
    body.textContent = content.text;
  } else content.render(body, view);
  dom.append(body);
  const actions = view.state
    .facet(popupActions)
    .filter((source) => source.kinds.includes(spec.kind))
    .flatMap((source) => source.actions(spec, view));
  if (actions.length > 0) {
    const row = document.createElement("div");
    row.className = "pw-lsp-popup-actions";
    for (const action of actions) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "pw-lsp-popup-action";
      button.dataset.popupAction = action.id;
      button.textContent = action.label;
      // 누름이 에디터의 포커스 · 선택을 빼앗지 않게 한다.
      button.addEventListener("mousedown", (e) => e.preventDefault());
      button.addEventListener("click", () => {
        closePopup(view);
        action.run(view);
      });
      row.append(button);
    }
    dom.append(row);
  }
  return dom;
}

/** 스크롤하면 닫는다(열려 있을 때만). */
const closeOnScroll = ViewPlugin.fromClass(
  class {
    private readonly onScroll = () => {
      if (currentPopup(this.view)) closePopup(this.view);
    };
    constructor(private readonly view: EditorView) {
      view.scrollDOM.addEventListener("scroll", this.onScroll, { passive: true });
    }
    destroy() {
      this.view.scrollDOM.removeEventListener("scroll", this.onScroll);
    }
  },
);

/** 열린 팝업만 닫는 Esc. 처리했다고 답해 keydown 이 `preventDefault` 된다(머리 주석). */
const closeOnEscape = Prec.highest(
  keymap.of([
    {
      key: "Escape",
      run: (view) => {
        if (!currentPopup(view)) return false;
        closePopup(view);
        return true;
      },
    },
  ]),
);

const popupTheme = EditorView.baseTheme({
  // 툴팁의 `dom` 이 곧 `.cm-tooltip` 이다(CodeMirror 가 클래스를 더한다).
  ".cm-tooltip.pw-lsp-popup": {
    border: "1px solid var(--pw-popup-border)",
    backgroundColor: "var(--pw-popup-background)",
    color: "var(--pw-popup-foreground)",
    borderRadius: "6px",
    boxShadow: "0 4px 14px rgb(0 0 0 / 0.18)",
    maxWidth: "min(640px, 80vw)",
    fontSize: "12px",
    lineHeight: "1.5",
  },
  ".pw-lsp-popup-body": {
    padding: "6px 10px",
    maxHeight: "min(360px, 50vh)",
    overflow: "auto",
  },
  ".pw-lsp-popup-markdown p": { margin: "4px 0" },
  ".pw-lsp-popup-markdown pre": {
    margin: "4px 0",
    padding: "4px 6px",
    borderRadius: "4px",
    backgroundColor: "var(--pw-popup-codeBackground)",
    whiteSpace: "pre-wrap",
    fontFamily: "var(--pw-editor-font-family, ui-monospace, monospace)",
  },
  ".pw-lsp-popup-markdown code": {
    fontFamily: "var(--pw-editor-font-family, ui-monospace, monospace)",
  },
  ".pw-lsp-popup-markdown a": { color: "var(--pw-ui-accent)", cursor: "pointer" },
  ".pw-lsp-popup-actions": {
    display: "flex",
    gap: "6px",
    padding: "4px 10px 6px",
    borderTop: "1px solid var(--pw-popup-border)",
  },
  ".pw-lsp-popup-action": {
    border: "none",
    background: "none",
    padding: "0",
    color: "var(--pw-ui-accent)",
    cursor: "pointer",
    font: "inherit",
  },
  ".pw-lsp-popup-action:focus-visible": {
    outline: "2px solid var(--pw-ui-accent)",
    outlineOffset: "1px",
  },
});

/** 팝업 바탕의 확장. LSP 기능 칸(src/editor/extensions/lsp.ts)이 문서마다 한 번 넣는다. */
export const popupBase = [popupField, closeOnEscape, closeOnScroll, popupTheme];
