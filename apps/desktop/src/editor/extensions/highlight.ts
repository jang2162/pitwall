// 구문 강조(12-m1-plan M1-17): 구문 분석 서비스(M1-10)의 강조 구간을 CodeMirror decoration 으로 그린다. 문서 세션은
// ../syntax-sessions.ts 가 갖고(`context.syntax`), 이 기능은 뷰마다 화면 범위만 묻는다.
//
// - 화면 범위(`view.viewport` — CodeMirror 가 그리는 줄들, 위아래 여유 포함)만 묻고, 스크롤해 받은 범위 밖으로 나가면 다시 묻는다.
// - 편집하면 옛 강조를 바뀜에 맞춰 옮겨 두고(`DecorationSet.map`) 다시 묻는다 — 응답 전에는 옛 강조가 남는다. 응답의 버전이 그사이
//   또 바뀌었으면 세션이 위치를 옮겨 준 것(`exact: false`)을 칠하고 다시 묻는다. 한 뷰에 묻는 요청은 하나만 띄우고, 그동안 생긴
//   일은 응답 뒤에 한 번 더 묻는 것으로 모은다.
// - 색은 토큰마다 CSS 클래스(`pw-syntax-<토큰>`)이고 클래스의 색은 테마 토큰의 CSS 변수(`--pw-syntax-<토큰>`, src/theme/tokens.ts)
//   다 — 테마 · 외관이 바뀌면 다시 묻지 않고 색만 바뀐다.
// - 켜는 조건: 세션이 있고(창이 Host 에 붙었다), 언어가 플레인 텍스트가 아니고(`PLAIN_TEXT` · `null`), 판정이 강조를 허락한다(`highlight` — 대용량
//   기준, 결정 M1-D9). 문법이 없는 언어 · 서버가 끈 문서는 세션이 `plain` 으로 답하고 이 뷰는 묻기를 멈춘다.
//
// 확장 값은 모듈 상수 하나라 다시 구성(외관 · 언어 바뀜)해도 플러그인이 그대로 남는다 — 언어 · 경로가 바뀐 것은 플러그인이
// `editorContext` 로 보고 세션을 바꿔 잡는다.

import { RangeSetBuilder, StateEffect } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  ViewPlugin,
  type ViewUpdate,
} from "@codemirror/view";
import type { HighlightSpan, SyntaxToken } from "@pitwall/rpc-client";

import { SYNTAX_TOKENS, syntaxTokenName, tokenCssVar } from "../../theme/tokens";
import { PLAIN_TEXT } from "../language";
import type { SyntaxHandle, SyntaxSessions } from "../syntax-sessions";
import { type EditorFeature, type EditorFeatureContext, editorContext } from ".";

/** 토큰의 CSS 클래스: `function.method` → `pw-syntax-function-method`. */
export const tokenClass = (token: SyntaxToken) => `pw-syntax-${token.replaceAll(".", "-")}`;

// 객체가 아니라 Map 이다 — 토큰 `constructor` 가 객체에서는 `Object.prototype.constructor` 를 집는다.
const MARKS = new Map<string, Decoration>(
  SYNTAX_TOKENS.map((token) => [token, Decoration.mark({ class: tokenClass(token) })]),
);

/** 글자 모양까지 바꾸는 토큰. 나머지는 색만. */
const SHAPES = new Map<SyntaxToken, Record<string, string>>([
  ["markup.bold", { fontWeight: "bold" }],
  ["markup.italic", { fontStyle: "italic" }],
  ["markup.heading", { fontWeight: "bold" }],
  ["markup.link.url", { textDecoration: "underline" }],
]);

/** 토큰 클래스마다의 스타일(에디터 테마의 규칙). */
export const highlightStyles: Record<string, Record<string, string>> = Object.fromEntries(
  SYNTAX_TOKENS.map((token) => [
    `.${tokenClass(token)}`,
    { color: `var(${tokenCssVar(syntaxTokenName(token))})`, ...SHAPES.get(token) },
  ]),
);

const highlightTheme = EditorView.theme(highlightStyles);

/** 겹치지 않고 차례인 구간을 decoration 으로. 문서 끝을 넘는 것은 자른다. */
export function spanDecorations(spans: readonly HighlightSpan[], length: number): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  for (const { from, to, token } of spans) {
    const mark = MARKS.get(token);
    const end = Math.min(to, length);
    // 모르는 토큰(더 새 서버)은 칠하지 않는다.
    if (mark && from < end) builder.add(from, end, mark);
  }
  return builder.finish();
}

/** 강조를 새로 받았다 — 뷰가 decoration 을 다시 읽게 하는 빈 트랜잭션의 표시. */
const refreshed = StateEffect.define<null>();

interface Binding {
  readonly syntax: SyntaxSessions | undefined;
  readonly path: string;
  readonly language: string | null;
}

const bindingOf = (c: EditorFeatureContext | undefined): Binding => ({
  syntax: c?.syntax,
  path: c?.path ?? "",
  language: c?.language ?? null,
});

const sameBinding = (a: Binding, b: Binding) =>
  a.syntax === b.syntax && a.path === b.path && a.language === b.language;

class Highlighter {
  decorations: DecorationSet = Decoration.none;
  private binding: Binding;
  private handle: SyntaxHandle | undefined;
  /** 지금 강조가 맞는 범위. 편집하면 지운다. */
  private covered: { from: number; to: number } | undefined;
  /** 이 문서는 강조하지 않는다(세션의 `plain`). */
  private plain = false;
  private running = false;
  /** 요청이 떠 있는 동안 다시 물을 일이 생겼다. */
  private again = false;
  private scheduled = false;
  private retry: ReturnType<typeof setTimeout> | undefined;
  private destroyed = false;

  constructor(private readonly view: EditorView) {
    this.binding = bindingOf(view.state.facet(editorContext));
    this.acquire();
  }

  private acquire() {
    const { syntax, path, language } = this.binding;
    this.handle = syntax && language ? syntax.acquire(path, language) : undefined;
    this.plain = false;
    this.covered = undefined;
    this.schedule();
  }

  update(u: ViewUpdate) {
    const binding = bindingOf(u.state.facet(editorContext));
    if (!sameBinding(binding, this.binding)) {
      this.handle?.release();
      this.binding = binding;
      this.decorations = Decoration.none;
      this.acquire();
      return;
    }
    if (u.docChanged) {
      this.decorations = this.decorations.map(u.changes);
      this.covered = undefined;
      this.schedule();
    } else if (u.viewportChanged && !this.covers()) this.schedule();
  }

  private covers() {
    const { from, to } = this.view.viewport;
    return this.covered !== undefined && from >= this.covered.from && to <= this.covered.to;
  }

  /** 다음 마이크로태스크에 묻는다 — 편집이 문서 모델을 거쳐 세션의 버전을 올린 뒤다(../live-text.ts `dispatch`). */
  private schedule() {
    if (!this.handle || this.plain || this.destroyed) return;
    if (this.running) {
      this.again = true;
      return;
    }
    if (this.scheduled) return;
    this.scheduled = true;
    clearTimeout(this.retry);
    this.retry = undefined;
    queueMicrotask(() => {
      this.scheduled = false;
      void this.run();
    });
  }

  private async run() {
    const handle = this.handle;
    if (!handle || this.plain || this.destroyed || this.running) return;
    this.running = true;
    this.again = false;
    const { from, to } = this.view.viewport;
    let answer: Awaited<ReturnType<SyntaxHandle["highlights"]>>;
    try {
      answer = await handle.highlights({ from, to });
    } catch (e) {
      console.warn("[editor] highlight request failed", e);
      answer = { kind: "stale" };
    } finally {
      this.running = false;
    }
    if (this.destroyed) return;
    if (handle !== this.handle) {
      // 그사이 언어 · 경로가 바뀌어 다른 세션을 잡았다.
      this.schedule();
      return;
    }
    if (answer.kind === "plain") {
      this.plain = true;
      if (this.decorations.size > 0) {
        this.decorations = Decoration.none;
        this.view.dispatch({ effects: refreshed.of(null) });
      }
      return;
    }
    if (answer.kind === "spans" && answer.version === handle.version) {
      this.decorations = spanDecorations(answer.spans, this.view.state.doc.length);
      this.view.dispatch({ effects: refreshed.of(null) });
      if (answer.exact) {
        this.covered = { from, to };
        if (this.again || !this.covers()) this.schedule();
        return;
      }
      // 지난 버전에서 옮긴 강조다 — 칠해 두고 다시 묻는다.
      this.covered = undefined;
    }
    // 버전이 지났다. 그사이 편집 · 스크롤이 있었으면 곧바로, 아니면(서버가 다시 여는 중 등) 다음 태스크에 다시 묻는다.
    if (this.again) this.schedule();
    else this.retry = setTimeout(() => this.schedule(), 0);
  }

  destroy() {
    this.destroyed = true;
    clearTimeout(this.retry);
    this.handle?.release();
    this.handle = undefined;
  }
}

const highlighter = ViewPlugin.fromClass(Highlighter, { decorations: (v) => v.decorations });

const highlightExtension = [highlighter, highlightTheme];

export const highlight: EditorFeature = {
  id: "highlight",
  extension: (c) =>
    c.syntax && c.language && c.language !== PLAIN_TEXT && c.judgment.highlight
      ? highlightExtension
      : [],
};
