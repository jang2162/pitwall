// 아웃라인 모델(15-m4-plan M4-10): 구조 툴 윈도우 · 파일 구조 팝업(⌘F12)이 함께 보는 심볼 트리. 출처가 둘이다 — 언어 서버
// (`lsp.symbols.document`, crates/pw-lsp/src/symbols.rs)와 심볼 인덱스(`index.file.symbols`, crates/pw-index — 서버가 없거나 준비
// 전 · 미신뢰일 때). 둘 다 평면 목록 + 부모 색인(부모가 자식보다 앞)으로 오고, 여기서 같은 트리(`Outline`)로 만든다.
//
// 위치는 CodeMirror 의 UTF-16 오프셋이다. 서버 답은 Host 가 오프셋으로 바꿔 오고, 인덱스 답(줄 · UTF-16 글자)은 요청에 실은
// 텍스트로 바꾼다. 이름 · 세부는 서버가 준 글 그대로다 — 화면은 글로만 그린다(HTML 로 해석하지 않는다).

import type { Text } from "@codemirror/state";
import { searchKey } from "@pitwall/keymap";
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

/**
 * 그리는 쪽의 상한 — 출처(언어 서버 · 심볼 인덱스)와 무관하게 `build` 한 곳에서 건다. 값은 Host 의 상한과 같다
 * (crates/pw-lsp/src/symbols.rs `MAX_SYMBOLS` · `MAX_DEPTH` · `MAX_TEXT`, 인덱스의 파일 상한 crates/pw-index/src/extract.rs
 * `MAX_SYMBOLS_PER_FILE`). 서버 답은 Host 가 이미 잘라 오지만, 인덱스 답은 깊이를 자르지 않는다(깊이 수천의 중첩 파일 하나가
 * 미신뢰 저장소에서 그대로 온다) — 그래서 여기서 한 번 더 건다.
 */
export const MAX_SYMBOLS = 20_000;
/** 맨 위가 깊이 0 — 깊이 `MAX_DEPTH - 1` 까지 둔다(32 단). */
export const MAX_DEPTH = 32;
/** 이름 · 세부의 UTF-8 바이트(글자 경계에서 자르고 `…`). */
export const MAX_TEXT_BYTES = 256;

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

/** `MAX_TEXT_BYTES` 바이트까지(글자 경계에서). 넘으면 끝에 `…`(Host 의 `clip` 과 같다). */
export function clipText(text: string): string {
  // UTF-8 은 UTF-16 단위 하나에 3 바이트를 넘지 않는다 — 대부분의 이름은 세지 않고 지나간다.
  if (text.length * 3 <= MAX_TEXT_BYTES) return text;
  let bytes = 0;
  let end = 0;
  for (const ch of text) {
    const code = ch.codePointAt(0)!;
    const size = code < 0x80 ? 1 : code < 0x800 ? 2 : code < 0x10000 ? 3 : 4;
    if (bytes + size > MAX_TEXT_BYTES) return `${text.slice(0, end)}…`;
    bytes += size;
    end += ch.length;
  }
  return text;
}

/**
 * 평면 목록 → 트리. 부모 색인이 자기보다 뒤이거나 없는 것을 가리키면 맨 위로 둔다. 상한(수 · 깊이 · 글)을 여기서 건다: 앞에서부터
 * `MAX_SYMBOLS` 개, 깊이 `MAX_DEPTH` 를 넘는 심볼(과 그 자손)은 버리고, 잘랐으면 `truncated`.
 */
function build(
  source: OutlineSource,
  flat: readonly Flat[],
  extra: { server?: string; truncated: boolean },
): Outline {
  let truncated = extra.truncated;
  /** 입력 색인 → 남긴 색인(버렸으면 -1). */
  const kept = new Int32Array(flat.length).fill(-1);
  const out: { s: Flat; parent?: number; depth: number }[] = [];
  for (let i = 0; i < flat.length; i++) {
    if (out.length >= MAX_SYMBOLS) {
      truncated = true;
      break;
    }
    const s = flat[i]!;
    const given = s.parent !== undefined && s.parent < i && s.parent >= 0 ? s.parent : undefined;
    let parent: number | undefined;
    let depth = 0;
    if (given !== undefined) {
      parent = kept[given]!;
      // 부모를 버렸으면(깊이) 자손도 버린다.
      if (parent < 0) continue;
      depth = out[parent]!.depth + 1;
      if (depth >= MAX_DEPTH) {
        truncated = true;
        continue;
      }
    }
    kept[i] = out.length;
    out.push({ s, ...(parent !== undefined && { parent }), depth });
  }
  const children: number[][] = out.map(() => []);
  const roots: number[] = [];
  out.forEach((o, i) => {
    if (o.parent === undefined) roots.push(i);
    else children[o.parent]!.push(i);
  });
  const symbols = out.map(({ s, parent, depth }, i): OutlineSymbol => ({
    id: i,
    name: clipText(s.name),
    ...(s.detail !== undefined && { detail: clipText(s.detail) }),
    kind: s.kind,
    deprecated: s.deprecated,
    ...(parent !== undefined && { parent }),
    depth,
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
    truncated,
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
    // 인덱스는 파일 하나의 심볼을 `MAX_SYMBOLS` 에서 조용히 자른다 — 그만큼 왔으면 잘린 것으로 본다.
    { truncated: answer.symbols.length >= MAX_SYMBOLS },
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

/**
 * 화면에 보이는 줄(깊이 우선, 접은 심볼의 자식은 뺀다). 재귀 대신 명시적 스택으로 훑는다 — 렌더 중에 불리므로 상한이 빠진 깊은
 * 트리에서도 스택이 넘쳐 창 전체가 언마운트되지 않게.
 */
export function visibleRows(
  outline: Outline,
  sort: OutlineSort,
  collapsed: ReadonlySet<number>,
): OutlineRow[] {
  const rows: OutlineRow[] = [];
  const stack = [...ordered(outline, outline.roots, sort)].reverse();
  while (stack.length > 0) {
    const id = stack.pop()!;
    const s = outline.symbols[id]!;
    rows.push({ id, depth: s.depth });
    if (s.children.length === 0 || collapsed.has(id)) continue;
    const kids = ordered(outline, s.children, sort);
    for (let k = kids.length - 1; k >= 0; k--) stack.push(kids[k]!);
  }
  return rows;
}

/**
 * 비교용 문자열(`searchKey` 와 같다). ASCII 만의 이름(코드 심볼 대부분)은 소문자로 바꾸기만 하면 같으므로 그 길로 간다 — 트리가
 * 바뀔 때 2만 이름을 자모로 푸는 시간을 줄인다.
 */
function nameKey(name: string): string {
  // eslint-disable-next-line no-control-regex
  return /^[\x00-\x7f]*$/.test(name) ? name.toLowerCase() : searchKey(name);
}

/** 질의의 비교용 문자열(공백을 빼고 자모로 푼 것 — `searchKey`). */
function queryKey(query: string): string {
  return nameKey(query.replace(/\s+/g, ""));
}

/** `q` 의 글자가 `t` 에 차례로 드는가(둘 다 `searchKey` 를 지난 것). */
function subsequence(q: string, t: string): boolean {
  let at = 0;
  for (const c of q) {
    at = t.indexOf(c, at);
    if (at < 0) return false;
    at += c.length;
  }
  return true;
}

/**
 * 이름이 질의에 맞는가: 질의의 글자(공백을 뺀 자모 — `searchKey`, 액션 찾기와 같아 한글을 조합하는 동안에도 맞는다)가 이름에
 * 차례로 든다(퍼지 — `gtNm` 이 `getName` 에). 이어진 부분(`matchScore`)도 차례로 드는 것이므로 함께 맞는다. 대소문자는 가리지
 * 않는다.
 */
export function fuzzyMatches(name: string, query: string): boolean {
  return subsequence(queryKey(query), nameKey(name));
}

/**
 * 트리 하나의 거르기 준비물 — 트리가 바뀔 때 한 번 만든다(키 입력마다 2만 심볼의 검색 키를 새로 만들지 않는다, M4-10 리뷰 M1).
 * `last` 는 바로 앞 질의의 맞은 심볼이다: 질의가 그 뒤에 글자를 더한 것이면(한 글자씩 치는 보통의 경우 · 한글 조합) 맞는 것은
 * 앞의 맞은 것 안에만 있으므로 그것만 다시 본다.
 */
interface FilterIndex {
  /** 이름의 검색 키(`nameKey`). */
  readonly keys: readonly string[];
  /** 위치 순 트리 차례(모두 펼침). */
  readonly order: readonly OutlineRow[];
  last?: { key: string; matched: readonly number[] };
}

const filterIndexes = new WeakMap<Outline, FilterIndex>();

function filterIndex(outline: Outline): FilterIndex {
  let index = filterIndexes.get(outline);
  if (!index) {
    index = {
      keys: outline.symbols.map((s) => nameKey(s.name)),
      order: visibleRows(outline, "position", new Set()),
    };
    filterIndexes.set(outline, index);
  }
  return index;
}

export type FilterRow = OutlineRow & { readonly match: boolean };

/**
 * 파일 구조 팝업의 거르기: 이름이 질의에 퍼지로 맞는 심볼(`fuzzyMatches`)과 그 조상(흐리게 — 맞지 않음)을 트리 차례로. 질의가
 * 비면 모든 심볼. 줄은 앞에서 `limit` 개까지만 만들고(팝업이 그리는 만큼 — 키마다 2만 개의 줄 객체를 만들지 않는다) 전체 수는
 * `total` 로 준다. 검색 키 · 트리 차례는 트리마다 한 번 만들어 둔다(`filterIndex`).
 */
export function filterRows(
  outline: Outline,
  query: string,
  sort: OutlineSort,
  limit = Number.POSITIVE_INFINITY,
): { rows: FilterRow[]; total: number } {
  const index = filterIndex(outline);
  const key = queryKey(query);
  const order = sort === "position" ? index.order : visibleRows(outline, sort, new Set());
  const rows: FilterRow[] = [];
  if (key === "") {
    for (const r of order) {
      if (rows.length >= limit) break;
      rows.push({ ...r, match: true });
    }
    return { rows, total: order.length };
  }
  const last = index.last;
  const candidates: Iterable<number> =
    last && key.startsWith(last.key) ? last.matched : outline.symbols.keys();
  const matched: number[] = [];
  for (const id of candidates) if (subsequence(key, index.keys[id]!)) matched.push(id);
  index.last = { key, matched };
  // 0 = 뺌, 1 = 조상(흐리게), 2 = 맞음.
  const mark = new Uint8Array(outline.symbols.length);
  let total = 0;
  for (const id of matched) {
    mark[id] = 2;
    total++;
    for (let at = outline.symbols[id]!.parent; at !== undefined && mark[at] === 0;) {
      mark[at] = 1;
      total++;
      at = outline.symbols[at]!.parent;
    }
  }
  for (const r of order) {
    if (rows.length >= limit) break;
    if (mark[r.id]! > 0) rows.push({ ...r, match: mark[r.id] === 2 });
  }
  return { rows, total };
}

/**
 * 심볼마다 다시 읽어도 같은 열쇠(조상의 종류 · 이름 경로의 해시) — 편집 뒤 다시 읽은 트리에서 접힘 · 고름을 이어 둔다(색인은 위쪽에
 * 심볼이 하나 생기면 밀린다, M4-10 리뷰 L4). 같은 부모 아래 같은 종류 · 이름이 둘이면 열쇠가 같다 — 둘이 함께 접힐 뿐이다.
 */
export function symbolKeys(outline: Outline): readonly number[] {
  let keys = symbolKeyCache.get(outline);
  if (!keys) {
    const out: number[] = [];
    for (const s of outline.symbols) {
      const up = s.parent === undefined ? "" : out[s.parent]!;
      out.push(hash(`${up}\u0000${s.kind}\u0000${s.name}`));
    }
    symbolKeyCache.set(outline, (keys = out));
  }
  return keys;
}

const symbolKeyCache = new WeakMap<Outline, readonly number[]>();

/** 53 비트 문자열 해시(cyrb53). */
function hash(text: string): number {
  let h1 = 0xdeadbeef;
  let h2 = 0x41c6ce57;
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    h1 = Math.imul(h1 ^ c, 2654435761);
    h2 = Math.imul(h2 ^ c, 1597334677);
  }
  h1 = Math.imul(h1 ^ (h1 >>> 16), 2246822507) ^ Math.imul(h2 ^ (h2 >>> 13), 3266489909);
  h2 = Math.imul(h2 ^ (h2 >>> 16), 2246822507) ^ Math.imul(h1 ^ (h1 >>> 13), 3266489909);
  return 4294967296 * (2097151 & h2) + (h1 >>> 0);
}
