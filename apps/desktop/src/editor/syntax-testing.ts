// 시험용 구문 분석 서비스: crates/pw-syntax 의 문서 세션 규칙(methods.rs 머리 주석)을 흉내 낸다 — 편집마다 버전, 앞당겨 도착한
// 편집은 차례가 올 때까지 들고 있기, 지난 버전은 conflict, 모르는 문서는 not found, 강조는 도착한 때의 버전으로 계산. 요청과 응답에
// 따로 지연을 줄 수 있어(가짜 타이머) 프레임이 뒤섞여 도착하는 것을 만든다.

import {
  ERROR_CODE,
  type HighlightSpan,
  type SyntaxEditParams,
  type SyntaxHighlightsParams,
  type SyntaxOff,
  type SyntaxOpenParams,
  type TextChange,
} from "@pitwall/rpc-client";

import type { SyntaxSource } from "./syntax-sessions";

/** 작은 강조기: `//` 줄 주석, `"` 문자열(줄 끝까지 닫히지 않으면 줄 끝까지), 키워드 `fn` · `let` · `return`. */
export function miniHighlight(text: string): HighlightSpan[] {
  const spans: HighlightSpan[] = [];
  let i = 0;
  while (i < text.length) {
    const c = text[i];
    if (c === "/" && text[i + 1] === "/") {
      const end = text.indexOf("\n", i);
      const to = end < 0 ? text.length : end;
      spans.push({ from: i, to, token: "comment" });
      i = to;
    } else if (c === '"') {
      let j = i + 1;
      while (j < text.length && text[j] !== '"' && text[j] !== "\n") j++;
      const to = text[j] === '"' ? j + 1 : j;
      spans.push({ from: i, to, token: "string" });
      i = to;
    } else if (/[A-Za-z_]/.test(c!)) {
      let j = i;
      while (j < text.length && /\w/.test(text[j]!)) j++;
      if (["fn", "let", "return"].includes(text.slice(i, j)))
        spans.push({ from: i, to: j, token: "keyword" });
      i = j;
    } else i++;
  }
  return spans;
}

interface Doc {
  language: string;
  text: string;
  version: number;
  early: Map<number, TextChange[]>;
  off: SyntaxOff | null;
}

export interface FakeSyntaxOptions {
  /** 언어 → 강조기. 없는 언어는 문법이 없다(`grammar: null`). 기본은 `Mini` 하나. */
  grammars?: Record<string, (text: string) => HighlightSpan[]>;
  /** 요청이 도착하기까지 · 응답이 돌아오기까지의 지연(ms). 0 이면 마이크로태스크. */
  delay?: (method: string, leg: "request" | "response") => number;
}

const rpcError = (code: number, message: string) => Object.assign(new Error(message), { code });

function apply(text: string, changes: readonly TextChange[]): string {
  for (const { from, to, insert } of changes) {
    if (to > text.length || from > to) throw rpcError(ERROR_CODE.INVALID_PARAMS, "range");
    text = text.slice(0, from) + insert + text.slice(to);
  }
  return text;
}

export function fakeSyntaxServer(options: FakeSyntaxOptions = {}) {
  const grammars = options.grammars ?? { Mini: miniHighlight };
  const docs = new Map<string, Doc>();
  const calls: { method: string; params: unknown }[] = [];

  const after = (ms: number, run: () => void) =>
    ms > 0 ? setTimeout(run, ms) : queueMicrotask(run);

  /** 요청 지연 뒤 `handle` 을 돌리고, 응답 지연 뒤 결과를 돌려준다. */
  function call<T>(method: string, params: unknown, handle: () => T): Promise<T> {
    calls.push({ method, params });
    return new Promise<T>((resolve, reject) =>
      after(options.delay?.(method, "request") ?? 0, () => {
        let outcome: { ok: true; value: T } | { ok: false; error: unknown };
        try {
          outcome = { ok: true, value: handle() };
        } catch (error) {
          outcome = { ok: false, error };
        }
        after(options.delay?.(method, "response") ?? 0, () =>
          outcome.ok ? resolve(outcome.value) : reject(outcome.error),
        );
      }),
    );
  }

  const doc = (docId: string) => {
    const d = docs.get(docId);
    if (!d) throw rpcError(ERROR_CODE.NOT_FOUND, `no document ${docId}`);
    return d;
  };

  const source: SyntaxSource = {
    open: (params: SyntaxOpenParams) =>
      call("open", params, () => {
        if (!grammars[params.language]) {
          docs.delete(params.docId);
          return { grammar: null, off: null };
        }
        docs.set(params.docId, {
          language: params.language,
          text: params.text,
          version: 0,
          early: new Map(),
          off: null,
        });
        return { grammar: params.language.toLowerCase(), off: null };
      }),
    edit: (params: SyntaxEditParams) =>
      call("edit", params, () => {
        const d = doc(params.docId);
        if (params.version <= d.version || d.early.has(params.version))
          throw rpcError(ERROR_CODE.CONFLICT, "stale version");
        d.early.set(params.version, params.changes);
        for (let next = d.early.get(d.version + 1); next; next = d.early.get(d.version + 1)) {
          d.early.delete(d.version + 1);
          d.text = apply(d.text, next);
          d.version += 1;
        }
        return { off: d.off };
      }),
    highlights: (params: SyntaxHighlightsParams) =>
      call("highlights", params, () => {
        const d = doc(params.docId);
        if (d.off) return { version: d.version, spans: [], off: d.off };
        const { from, to } = params.range;
        const spans = grammars[d.language]!(d.text)
          .filter((s) => s.to > from && s.from < to)
          .map((s) => ({ ...s, from: Math.max(s.from, from), to: Math.min(s.to, to) }));
        return { version: d.version, spans, off: null };
      }),
    close: (params) =>
      call("close", params, () => {
        docs.delete(params.docId);
        return null;
      }),
  };

  return {
    source,
    calls,
    docs,
    /** 메서드의 호출들. */
    called: (method: string) => calls.filter((c) => c.method === method).map((c) => c.params),
    /** 서버가 문서를 잊는다(오래 안 쓴 문서를 닫음 — `MAX_DOCUMENTS`). */
    forget: (docId: string) => docs.delete(docId),
    /** 문서의 강조를 끈다(다음 편집 · 강조가 `off` 로 답한다). */
    turnOff: (docId: string, off: SyntaxOff) => (doc(docId).off = off),
  };
}
