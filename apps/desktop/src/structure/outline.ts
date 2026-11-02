// 아웃라인 모델(15-m4-plan M4-10): 구조 툴 윈도우 · 파일 구조 팝업(⌘F12)이 함께 보는 심볼 트리. 출처가 둘이다 — 언어 서버
// (`lsp.symbols.document`, crates/pw-lsp/src/symbols.rs)와 심볼 인덱스(`index.file.symbols`, crates/pw-index — 서버가 없거나 준비
// 전 · 미신뢰일 때). 둘 다 평면 목록 + 부모 색인(부모가 자식보다 앞)으로 오고, 여기서 같은 트리(`Outline`)로 만든다.
//
// 위치는 CodeMirror 의 UTF-16 오프셋이다. 서버 답은 Host 가 오프셋으로 바꿔 오고, 인덱스 답(줄 · UTF-16 글자)은 요청에 실은
// 텍스트로 바꾼다. 이름 · 세부는 서버가 준 글 그대로다 — 화면은 글로만 그린다(HTML 로 해석하지 않는다).

import type { Text } from "@codemirror/state";
import { matchScore, searchKey } from "@pitwall/keymap";
import type { IndexFileSymbols, IndexSymbolKind, LspDocumentSymbols } from "@pitwall/rpc-client";

/** 어디서 온 트리인가. 화면이 흐리게 적는다. */
export type OutlineSource = "lsp" | "index";

/**
 * 심볼 종류(아이콘 · 접근성 이름). LSP `SymbolKind` 와 인덱스 종류(`IndexSymbolKind`)를 모은 것이다 — 값 종류(문자열 · 숫자 …
 * JSON · YAML 의 키)는 `value` 하나로, 모르는 종류는 `symbol`.
 */
export type OutlineKind =
  | "module"
  | "class"
  | "method"
  | "property"
  | "field"
  | "constructor"
  | "enum"
  | "interface"
  | "function"
  | "variable"
  | "constant"
  | "value"
  | "enumMember"
  | "struct"
  | "event"
  | "operator"
  | "typeParameter"
  | "trait"
  | "typeAlias"
  | "macro"
  | "impl"
  | "symbol";

/** LSP `SymbolKind`(1 ~ 26) → 종류. 0(범위 밖)은 `symbol`. */
const LSP_KINDS: readonly OutlineKind[] = [
  "symbol",
  "module", // File
  "module",
  "module", // Namespace
  "module", // Package
  "class",
  "method",
  "property",
  "field",
  "constructor",
  "enum",
  "interface",
  "function",
  "variable",
  "constant",
  "value", // String
  "value", // Number
  "value", // Boolean
  "value", // Array
  "value", // Object
  "value", // Key
  "value", // Null
  "enumMember",
  "struct",
  "event",
  "operator",
  "typeParameter",
];

const INDEX_KINDS: Readonly<Record<IndexSymbolKind, OutlineKind>> = {
  module: "module",
  class: "class",
  struct: "struct",
  interface: "interface",
  trait: "trait",
  enum: "enum",
  enumMember: "enumMember",
  typeAlias: "typeAlias",
  function: "function",
  method: "method",
  constructor: "constructor",
  field: "field",
  property: "property",
  constant: "constant",
  variable: "variable",
  macro: "macro",
  impl: "impl",
};

export interface OutlineSymbol {
  /** 이 트리 안의 색인. */
  readonly id: number;
  readonly name: string;
  readonly detail?: string;
  readonly kind: OutlineKind;
  readonly deprecated: boolean;
  readonly parent?: number;
  /** 맨 위가 0. */
  readonly depth: number;
  readonly children: readonly number[];
  /** 심볼 전체(UTF-16 오프셋, 끝 제외). */
  readonly from: number;
  readonly to: number;
  /** 이름 자리 — 고르면 커서가 간다. */
  readonly selectionFrom: number;
  readonly selectionTo: number;
}

export interface Outline {
  readonly source: OutlineSource;
  /** 답한 서버 id(`source: "lsp"`). */
  readonly server?: string;
  /** 앞에서부터, 부모가 자식보다 앞. */
  readonly symbols: readonly OutlineSymbol[];
  readonly roots: readonly number[];
  /** 상한에 걸려 일부가 빠졌다. */
  readonly truncated: boolean;
}

interface Flat {
  name: string;
  detail?: string;
  kind: OutlineKind;
  deprecated: boolean;
  parent?: number;
  from: number;
  to: number;
  selectionFrom: number;
  selectionTo: number;
}

/** 평면 목록 → 트리. 부모 색인이 자기보다 뒤이거나 없는 것을 가리키면 맨 위로 둔다. */
function build(
  source: OutlineSource,
  flat: readonly Flat[],
  extra: { server?: string; truncated: boolean },
): Outline {
  const children: number[][] = flat.map(() => []);
  const roots: number[] = [];
  const depth: number[] = [];
  flat.forEach((s, i) => {
    const parent = s.parent !== undefined && s.parent < i && s.parent >= 0 ? s.parent : undefined;
    if (parent === undefined) {
      roots.push(i);
      depth.push(0);
    } else {
      children[parent]!.push(i);
      depth.push(depth[parent]! + 1);
    }
  });
  const symbols = flat.map((s, i): OutlineSymbol => ({
    id: i,
    name: s.name,
    ...(s.detail !== undefined && { detail: s.detail }),
    kind: s.kind,
    deprecated: s.deprecated,
    ...(depth[i]! > 0 && { parent: s.parent }),
    depth: depth[i]!,
    children: children[i]!,
    from: s.from,
    to: s.to,
    selectionFrom: s.selectionFrom,
    selectionTo: s.selectionTo,
  }));
  return {
    source,
    ...(extra.server !== undefined && { server: extra.server }),
    symbols,
    roots,
    truncated: extra.truncated,
  };
}

/** 언어 서버의 답(`lsp.symbols.document`). */
export function fromLsp(answer: LspDocumentSymbols): Outline {
  return build(
    "lsp",
    answer.symbols.map((s) => ({
      name: s.name,
      ...(s.detail !== null && { detail: s.detail }),
      kind: LSP_KINDS[s.kind] ?? "symbol",
      deprecated: s.deprecated,
      ...(s.parent !== null && { parent: s.parent }),
      from: s.from,
      to: s.to,
      selectionFrom: s.selectionFrom,
      selectionTo: s.selectionTo,
    })),
    { server: answer.server, truncated: answer.truncated },
  );
}

/** 줄(0부터) · UTF-16 글자 → 오프셋. 줄 · 글자가 텍스트 밖이면 끝으로 당긴다. */
function offsetOf(text: Text, line: number, character: number): number {
  if (line >= text.lines) return text.length;
  const at = text.line(line + 1);
  return at.from + Math.min(character, at.length);
}

/** 심볼 인덱스의 답(`index.file.symbols`) — `text` 는 요청에 실은 텍스트. */
export function fromIndex(answer: IndexFileSymbols, text: Text): Outline {
  return build(
    "index",
    answer.symbols.map((s) => {
      const from = offsetOf(text, s.range.start.line, s.range.start.character);
      const to = Math.max(from, offsetOf(text, s.range.end.line, s.range.end.character));
      const selectionFrom = offsetOf(
        text,
        s.selectionRange.start.line,
        s.selectionRange.start.character,
      );
      const selectionTo = offsetOf(text, s.selectionRange.end.line, s.selectionRange.end.character);
      const inside = from <= selectionFrom && selectionTo <= to && selectionFrom <= selectionTo;
      return {
        name: s.name,
        kind: INDEX_KINDS[s.kind] ?? "symbol",
        deprecated: false,
        ...(s.parent !== null && { parent: s.parent }),
        from,
        to,
        selectionFrom: inside ? selectionFrom : from,
        selectionTo: inside ? selectionTo : from,
      };
    }),
    { truncated: false },
  );
}

/** `pos` 를 담은 가장 안쪽 심볼(커서 따라가기). 끝 자리(`}` 바로 뒤)도 담은 것으로 본다. 없으면 `undefined`. */
export function innermostAt(outline: Outline, pos: number): number | undefined {
  let best: OutlineSymbol | undefined;
  for (const s of outline.symbols) {
    if (s.from > pos || pos > s.to) continue;
    if (
      !best ||
      s.depth > best.depth ||
      (s.depth === best.depth && s.to - s.from < best.to - best.from)
    )
      best = s;
  }
  return best?.id;
}

/** `id` 의 조상(가까운 것부터). */
export function ancestors(outline: Outline, id: number): number[] {
  const out: number[] = [];
  let at = outline.symbols[id]?.parent;
  while (at !== undefined) {
    out.push(at);
    at = outline.symbols[at]?.parent;
  }
  return out;
}

export type OutlineSort = "position" | "name";

const collator = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });

function ordered(outline: Outline, ids: readonly number[], sort: OutlineSort): readonly number[] {
  const at = (id: number) => outline.symbols[id]!;
  const byPosition = (a: number, b: number) => at(a).from - at(b).from || a - b;
  return [...ids].sort(
    sort === "name"
      ? (a, b) => collator.compare(at(a).name, at(b).name) || byPosition(a, b)
      : byPosition,
  );
}

export interface OutlineRow {
  readonly id: number;
  readonly depth: number;
}

/** 화면에 보이는 줄(깊이 우선, 접은 심볼의 자식은 뺀다). */
export function visibleRows(
  outline: Outline,
  sort: OutlineSort,
  collapsed: ReadonlySet<number>,
): OutlineRow[] {
  const rows: OutlineRow[] = [];
  const walk = (ids: readonly number[]) => {
    for (const id of ordered(outline, ids, sort)) {
      const s = outline.symbols[id]!;
      rows.push({ id, depth: s.depth });
      if (s.children.length > 0 && !collapsed.has(id)) walk(s.children);
    }
  };
  walk(outline.roots);
  return rows;
}

/**
 * 이름이 질의에 맞는가: 이어진 부분(`matchScore` — 자모 단위라 한글을 조합하는 동안에도 맞는다, 액션 찾기와 같다)이거나, 질의의
 * 글자가 이름에 차례로 든다(퍼지 — `gtNm` 이 `getName` 에). 대소문자는 가리지 않는다.
 */
export function fuzzyMatches(name: string, query: string): boolean {
  if (matchScore(name, query) > 0) return true;
  const q = searchKey(query.trim().replace(/\s+/g, ""));
  const t = searchKey(name);
  let at = 0;
  for (const c of q) {
    at = t.indexOf(c, at);
    if (at < 0) return false;
    at += c.length;
  }
  return true;
}

/**
 * 파일 구조 팝업의 거르기: 이름이 질의에 퍼지로 맞는 심볼(`fuzzyMatches`)과 그 조상(흐리게 — 맞지 않음)을 트리 차례로. 질의가
 * 비면 모든 심볼.
 */
export function filterRows(
  outline: Outline,
  query: string,
  sort: OutlineSort,
): (OutlineRow & { match: boolean })[] {
  const q = query.trim();
  const all = visibleRows(outline, sort, new Set());
  if (q === "") return all.map((r) => ({ ...r, match: true }));
  const keep = new Set<number>();
  const match = new Set<number>();
  for (const s of outline.symbols) {
    if (!fuzzyMatches(s.name, q)) continue;
    match.add(s.id);
    keep.add(s.id);
    for (const a of ancestors(outline, s.id)) keep.add(a);
  }
  return all.filter((r) => keep.has(r.id)).map((r) => ({ ...r, match: match.has(r.id) }));
}
