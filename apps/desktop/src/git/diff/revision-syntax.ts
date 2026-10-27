// 리비전 텍스트의 구문 강조(M1-17 재사용): 커밋 · index 의 파일 내용은 문서 모델(src/editor/documents.ts)에 없고 바뀌지 않으므로,
// 구문 분석 서비스에 그 텍스트로 문서 하나를 열고(`syntax.open`) 화면 범위만 묻는다(`syntax.highlights`). 편집이 없어 버전은 늘
// 0 이다. 구간 → decoration · 토큰 색은 에디터의 강조 기능(src/editor/extensions/highlight.ts)의 것을 그대로 쓴다.
//
// - 문서 id 는 창마다 다른 앞부분을 단다(같은 Host 의 다른 창 · 에디터 문서 세션과 겹치지 않게).
// - 문법이 없는 언어 · 서버가 끈 문서(`grammar: null`)는 묻지 않는다. 실패는 칠하지 않는 것으로 끝낸다(다시 묻지 않는다 — 텍스트가
//   바뀌지 않으므로 다시 열어도 같다. 연결이 끊긴 창은 탭을 다시 열면 된다).
// - 뷰가 사라지면 닫는다(`release`). 같은 텍스트를 두 뷰(나란히 보기의 한쪽 · 통합 보기의 뺀 줄 블록)가 함께 쓰면 마지막이 닫는다.

import { StateEffect } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  ViewPlugin,
  type ViewUpdate,
} from "@codemirror/view";
import type {
  HighlightSpan,
  SyntaxCloseParams,
  SyntaxHighlightsParams,
  SyntaxHighlightsResult,
  SyntaxOpenParams,
  SyntaxOpenResult,
} from "@pitwall/rpc-client";

import { highlightStyles, spanDecorations } from "../../editor/extensions/highlight";
import { PLAIN_TEXT } from "../../editor/language";

/** 구문 분석 서비스 RPC 중 바뀌지 않는 텍스트에 쓰는 것. */
export interface StaticSyntaxSource {
  open(params: SyntaxOpenParams): Promise<SyntaxOpenResult>;
  highlights(params: SyntaxHighlightsParams): Promise<SyntaxHighlightsResult>;
  close(params: SyntaxCloseParams): Promise<unknown>;
}

export interface RevisionSyntax {
  /** 범위(UTF-16 위치)의 강조 구간. 강조하지 않는 문서 · 실패면 빈 배열. */
  highlights(from: number, to: number): Promise<readonly HighlightSpan[]>;
  /** 잡는다. 놓는 함수를 돌려준다 — 마지막이 놓으면 서비스의 문서를 닫는다. */
  retain(): () => void;
}

const windowTag = Math.random().toString(36).slice(2, 8);
let seq = 0;

/** `text` 를 `language`(Linguist 이름)로 강조하는 세션. 언어가 없거나 플레인 텍스트면 `undefined`. */
export function revisionSyntax(
  source: StaticSyntaxSource,
  language: string | null,
  text: string,
): RevisionSyntax | undefined {
  if (!language || language === PLAIN_TEXT) return undefined;
  const docId = `rev-${windowTag}-${++seq}`;
  let holders = 0;
  let closed = false;
  const opened: Promise<boolean> = source.open({ docId, language, text }).then(
    (r) => r.grammar !== null,
    (e: unknown) => {
      console.warn("[git] revision highlight open failed", e);
      return false;
    },
  );
  return {
    async highlights(from, to) {
      if (!(await opened) || closed) return [];
      try {
        const r = await source.highlights({ docId, range: { from, to } });
        return r.spans;
      } catch (e) {
        console.warn("[git] revision highlight failed", e);
        return [];
      }
    },
    retain() {
      holders++;
      let released = false;
      return () => {
        if (released) return;
        released = true;
        if (--holders > 0) return;
        // 놓은 뒤 같은 커밋에서 다시 잡을 수 있다(뷰를 다시 만듦) — 한 마이크로태스크 기다린다.
        queueMicrotask(() => {
          if (holders > 0 || closed) return;
          closed = true;
          void opened.then((open) => {
            if (open) void source.close({ docId }).catch(() => {});
          });
        });
      };
    },
  };
}

const refreshed = StateEffect.define<null>();

/** 뷰의 화면 범위를 묻고 칠하는 확장. 받은 범위 밖으로 스크롤하면 다시 묻는다. */
export function revisionHighlight(session: RevisionSyntax | undefined) {
  if (!session) return [];
  const syntax = session;
  const plugin = ViewPlugin.fromClass(
    class {
      decorations: DecorationSet = Decoration.none;
      private covered: { from: number; to: number } | undefined;
      private asking = false;
      private release: () => void;
      private destroyed = false;

      constructor(private readonly view: EditorView) {
        this.release = syntax.retain();
        this.ask();
      }

      update(u: ViewUpdate) {
        if (u.viewportChanged && !this.covers()) this.ask();
      }

      private covers() {
        const { from, to } = this.view.viewport;
        return this.covered !== undefined && from >= this.covered.from && to <= this.covered.to;
      }

      private ask() {
        if (this.asking || this.destroyed) return;
        this.asking = true;
        const { from, to } = this.view.viewport;
        void syntax.highlights(from, to).then((spans) => {
          this.asking = false;
          if (this.destroyed) return;
          this.covered = { from, to };
          this.decorations = spanDecorations(spans, this.view.state.doc.length);
          this.view.dispatch({ effects: refreshed.of(null) });
          if (!this.covers()) this.ask();
        });
      }

      destroy() {
        this.destroyed = true;
        this.release();
      }
    },
    { decorations: (v) => v.decorations },
  );
  return [plugin, EditorView.theme(highlightStyles)];
}
