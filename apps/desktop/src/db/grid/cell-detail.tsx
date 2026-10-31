// 셀 상세 패널(M3-16): 고른 셀 하나의 값 전부를 보인다 — JSON 은 접는 트리, 긴 글은 줄바꿈한 글, 바이트는 hex 덤프, 셀 상한에서
// 잘린 값은 원래 크기를 함께. 값은 신뢰할 수 없는 글이라 React 의 글자 노드로만 그린다(HTML 로 해석하는 길을 두지 않는다).

import type { ColumnMeta, DbValue } from "@pitwall/rpc-client";
import { useMemo, useState } from "react";

import type { DbGridStrings } from "./strings";
import { isTruncated, originalSize, valueKind, valueText } from "./values";

/** 트리의 한 마디에서 처음 펼쳐 보이는 자식 수. 나머지는 「… N개 더」 를 눌러 본다. */
export const TREE_PAGE = 200;
/** hex 덤프의 한 줄 바이트 수. */
const HEX_ROW = 16;
/** hex 덤프로 그리는 최대 바이트(넘으면 앞부분만). */
const HEX_LIMIT = 64 * 1024;

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MiB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GiB`;
}

function utf8Length(s: string): number {
  return new TextEncoder().encode(s).length;
}

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

function JsonNode({
  name,
  value,
  depth,
  T,
}: {
  name?: string;
  value: Json;
  depth: number;
  T: DbGridStrings;
}) {
  const [open, setOpen] = useState(depth < 1);
  const [shown, setShown] = useState(TREE_PAGE);
  const label =
    name === undefined ? null : <span className="text-ui-muted-foreground">{name}: </span>;
  if (value === null || typeof value !== "object") {
    const text = typeof value === "string" ? JSON.stringify(value) : String(value);
    return (
      <div className="whitespace-pre-wrap break-all pl-4" data-json-leaf>
        {label}
        <span>{text}</span>
      </div>
    );
  }
  const entries: [string, Json][] = Array.isArray(value)
    ? value.map((v, i) => [String(i), v])
    : Object.entries(value);
  const brackets = Array.isArray(value) ? ["[", "]"] : ["{", "}"];
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
          {entries.slice(0, shown).map(([k, v]) => (
            <JsonNode key={k} name={k} value={v} depth={depth + 1} T={T} />
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
  const parsed = useMemo((): { ok: true; json: Json } | { ok: false } | undefined => {
    if (value === undefined || value === "null") return undefined;
    const raw = value.json;
    if (raw === undefined) return undefined;
    try {
      return { ok: true, json: JSON.parse(raw) as Json };
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
      <div className="min-h-0 flex-1 overflow-auto p-2 font-mono text-xs" data-view={view}>
        {value === "null" ? (
          <span className="text-ui-muted-foreground">NULL</span>
        ) : view === "json" && parsed?.ok ? (
          <JsonNode value={parsed.json} depth={0} T={T} />
        ) : view === "hex" && value.bytes !== undefined ? (
          <pre className="whitespace-pre">{hexDump(value.bytes.hex)}</pre>
        ) : (
          <>
            {view === "json" && parsed && !parsed.ok && (
              <div className="mb-1 text-ui-muted-foreground">{T.detail.jsonInvalid}</div>
            )}
            <pre className="whitespace-pre-wrap break-all">{text}</pre>
          </>
        )}
      </div>
    </div>
  );
}
