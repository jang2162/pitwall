// 셀 상세 패널(M3-16): 고른 셀 하나의 값 전부를 보인다 — JSON 은 접는 트리, 긴 글은 줄바꿈한 글, 바이트는 hex 덤프, 셀 상한에서
// 잘린 값은 원래 크기를 함께. 값은 신뢰할 수 없는 글이라 React 의 글자 노드로만 그린다(HTML 로 해석하는 길을 두지 않는다).
// JSON 트리는 원래 글을 지키는 파서(json-text.ts)로 읽는다 — 수는 받은 글 그대로(큰 정수 · 긴 소수 · `-0` · 지수), 같은 키도 모두
// 보인다. 보이지 않는 제어 · 방향 · 0폭 문자는 `⟨U+XXXX⟩` 로 보인다(values.ts 의 `showInvisible`). 표지는 보이기만 하는 것이다
// — 고른 글을 복사하면(⌘C · 우클릭 「복사」 모두 `copy` 이벤트) 표지 대신 원래 글자가 간다(`copyOriginal`).

import type { ColumnMeta, DbValue } from "@pitwall/rpc-client";
import { type ClipboardEvent, useMemo, useState } from "react";

import { type JsonText, parseJsonText } from "./json-text";
import type { DbGridStrings } from "./strings";
import {
  formatSize,
  isTruncated,
  originalSize,
  originalSlice,
  showInvisible,
  valueKind,
  valueText,
} from "./values";

/** 트리의 한 마디에서 처음 펼쳐 보이는 자식 수. 나머지는 「… N개 더」 를 눌러 본다. */
export const TREE_PAGE = 200;
/** hex 덤프의 한 줄 바이트 수. */
const HEX_ROW = 16;
/** hex 덤프로 그리는 최대 바이트(넘으면 앞부분만). */
const HEX_LIMIT = 64 * 1024;

function utf8Length(s: string): number {
  return new TextEncoder().encode(s).length;
}

/** `Shown` 이 그린 요소 → 원래 글. 글자 노드는 그 요소의 하나뿐인 자식이다. */
const originals = new WeakMap<Element, string>();

/** 원래 글 `text` 를 표지를 넣어 그린다. 복사는 `copyOriginal` 이 원래 글로 되돌린다. */
function Shown({ text }: { text: string }) {
  return (
    <span
      ref={(el) => {
        if (el) originals.set(el, text);
      }}
    >
      {showInvisible(text)}
    </span>
  );
}

/**
 * 셀 상세에서 고른 글을 복사할 때 표지(`⟨U+XXXX⟩`)를 원래 글자로 바꿔 쓴다. 고른 곳에 표지가 없으면 웹뷰의 복사 그대로 둔다.
 * 블록(`div` · `pre`)이 바뀌는 자리에 줄바꿈을 넣는다(JSON 트리의 한 줄이 블록 하나).
 */
function copyOriginal(e: ClipboardEvent<HTMLElement>) {
  const selection = document.getSelection();
  if (!selection || selection.isCollapsed || selection.rangeCount === 0) return;
  const scope = e.currentTarget;
  const parts: string[] = [];
  let marked = false;
  let block: Element | null = null;
  for (let r = 0; r < selection.rangeCount; r++) {
    const range = selection.getRangeAt(r);
    const top = range.commonAncestorContainer;
    if (!scope.contains(top)) return;
    const texts: Text[] = [];
    if (top instanceof Text) texts.push(top);
    else {
      const walker = document.createTreeWalker(top, NodeFilter.SHOW_TEXT);
      for (let n = walker.nextNode(); n; n = walker.nextNode()) {
        if (range.intersectsNode(n)) texts.push(n as Text);
      }
    }
    for (const node of texts) {
      const from = node === range.startContainer ? range.startOffset : 0;
      const to = node === range.endContainer ? range.endOffset : node.data.length;
      if (from >= to) continue;
      const parent = node.parentElement;
      const here = parent?.closest("div, pre") ?? null;
      if (block !== null && here !== block) parts.push("\n");
      block = here;
      const raw = parent ? originals.get(parent) : undefined;
      if (raw !== undefined && raw !== node.data && node.data === showInvisible(raw)) {
        marked = true;
        parts.push(originalSlice(raw, node.data, from, to));
      } else parts.push(node.data.slice(from, to));
    }
  }
  if (!marked) return;
  e.preventDefault();
  e.clipboardData.setData("text/plain", parts.join(""));
}

function JsonNode({
  name,
  value,
  depth,
  T,
}: {
  name?: string;
  value: JsonText;
  depth: number;
  T: DbGridStrings;
}) {
  const [open, setOpen] = useState(depth < 1);
  const [shown, setShown] = useState(TREE_PAGE);
  const label =
    name === undefined ? null : (
      <span className="text-ui-muted-foreground">
        <Shown text={name} />:{" "}
      </span>
    );
  if (value.kind !== "array" && value.kind !== "object") {
    const text =
      value.kind === "string"
        ? JSON.stringify(value.value)
        : value.kind === "number"
          ? value.raw
          : value.kind === "bool"
            ? String(value.value)
            : "null";
    return (
      <div className="whitespace-pre-wrap break-all pl-4" data-json-leaf>
        {label}
        <Shown text={text} />
      </div>
    );
  }
  const entries: [string, JsonText][] =
    value.kind === "array" ? value.items.map((v, i) => [String(i), v]) : value.entries;
  const brackets = value.kind === "array" ? ["[", "]"] : ["{", "}"];
  return (
    <div className="pl-4" data-json-node>
      <button
        type="button"
        className="-ml-4 w-4 text-ui-muted-foreground"
        aria-label={open ? T.detail.collapse : T.detail.expand}
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        {open ? "▾" : "▸"}
      </button>
      {label}
      <span className="text-ui-muted-foreground">
        {brackets[0]}
        {open ? "" : ` ${entries.length} ${brackets[1]}`}
      </span>
      {open && (
        <>
          {entries.slice(0, shown).map(([k, v], i) => (
            // 객체는 같은 키가 여럿일 수 있다 — 자리로 키를 삼는다.
            <JsonNode key={i} name={k} value={v} depth={depth + 1} T={T} />
          ))}
          {entries.length > shown && (
            <button
              type="button"
              className="pl-4 text-ui-accent"
              onClick={() => setShown(shown + TREE_PAGE)}
            >
              {T.detail.more(entries.length - shown)}
            </button>
          )}
          <div className="text-ui-muted-foreground">{brackets[1]}</div>
        </>
      )}
    </div>
  );
}

function hexBytes(hex: string): number {
  return (hex.startsWith("0x") ? hex.length - 2 : hex.length) / 2;
}

function hexDump(hex: string): string {
  const digits = hex.startsWith("0x") ? hex.slice(2) : hex;
  const lines: string[] = [];
  const bytes = Math.min(digits.length / 2, HEX_LIMIT);
  for (let off = 0; off < bytes; off += HEX_ROW) {
    const chunk: string[] = [];
    let ascii = "";
    for (let i = off; i < Math.min(off + HEX_ROW, bytes); i++) {
      const b = digits.slice(i * 2, i * 2 + 2);
      chunk.push(b);
      const code = Number.parseInt(b, 16);
      ascii += code >= 0x20 && code < 0x7f ? String.fromCharCode(code) : ".";
    }
    lines.push(
      `${off.toString(16).padStart(8, "0")}  ${chunk.join(" ").padEnd(HEX_ROW * 3 - 1)}  ${ascii}`,
    );
  }
  return lines.join("\n");
}

type View = "text" | "json" | "hex";

export function CellDetail({
  value,
  column,
  T,
}: {
  value: DbValue | undefined;
  column: ColumnMeta | undefined;
  T: DbGridStrings;
}) {
  const kind = value === undefined ? undefined : valueKind(value);
  const text = value === undefined ? null : valueText(value);
  const parsed = useMemo((): { ok: true; json: JsonText } | { ok: false } | undefined => {
    if (value === undefined || value === "null") return undefined;
    const raw = value.json;
    if (raw === undefined) return undefined;
    try {
      return { ok: true, json: parseJsonText(raw) };
    } catch {
      return { ok: false };
    }
  }, [value]);
  const views: View[] =
    kind === "json" ? ["json", "text"] : kind === "bytes" ? ["hex", "text"] : ["text"];
  const [picked, setPicked] = useState<View | undefined>();
  const view = picked && views.includes(picked) ? picked : views[0]!;

  if (value === undefined) {
    return (
      <div className="p-3 text-ui-muted-foreground" data-testid="db-cell-detail">
        {T.detail.none}
      </div>
    );
  }
  const size = originalSize(value);
  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="db-cell-detail" data-kind={kind}>
      <div className="flex items-center gap-2 border-b border-ui-border px-2 py-1 text-xs">
        <span className="font-semibold">{column?.name}</span>
        <span className="text-ui-muted-foreground">{column?.dbType}</span>
        <span className="flex-1" />
        {views.length > 1 &&
          views.map((v) => (
            <button
              key={v}
              type="button"
              aria-pressed={view === v}
              className={view === v ? "font-semibold text-ui-accent" : "text-ui-muted-foreground"}
              onClick={() => setPicked(v)}
            >
              {T.detail[v]}
            </button>
          ))}
      </div>
      {isTruncated(value) && size !== undefined && (
        <div
          className="border-b border-ui-border px-2 py-1 text-xs"
          data-testid="db-cell-truncated"
        >
          {T.detail.truncated(
            formatSize(size),
            formatSize(
              value !== "null" && value.bytes !== undefined
                ? (value.bytes.hex.length - 2) / 2
                : utf8Length(text ?? ""),
            ),
          )}
        </div>
      )}
      <div
        className="min-h-0 flex-1 overflow-auto p-2 font-mono text-xs"
        data-view={view}
        data-testid="db-cell-detail-body"
        onCopy={copyOriginal}
      >
        {value === "null" ? (
          <span className="text-ui-muted-foreground">NULL</span>
        ) : view === "json" && parsed?.ok ? (
          <JsonNode value={parsed.json} depth={0} T={T} />
        ) : view === "hex" && value.bytes !== undefined ? (
          <>
            {hexBytes(value.bytes.hex) > HEX_LIMIT && (
              <div className="mb-1 text-ui-muted-foreground" data-testid="db-cell-hex-partial">
                {T.detail.hexPartial(formatSize(HEX_LIMIT), formatSize(hexBytes(value.bytes.hex)))}
              </div>
            )}
            <pre className="whitespace-pre">{hexDump(value.bytes.hex)}</pre>
          </>
        ) : (
          <>
            {view === "json" && parsed && !parsed.ok && (
              <div className="mb-1 text-ui-muted-foreground">{T.detail.jsonInvalid}</div>
            )}
            <pre className="whitespace-pre-wrap break-all">
              <Shown text={text ?? ""} />
            </pre>
          </>
        )}
      </div>
    </div>
  );
}
