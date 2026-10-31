// 값 뷰어(14-m3-plan M3-18): 고른 키 하나의 값을 쪽 단위로(`db.kv.get { key, range }` — 쪽 하나는 Host 의 4 MiB 예산, 넘으면
// `nextCursor` 가 다음 쪽을 가리킨다). 머리에 타입 · TTL(남은 시간, 초마다) · 크기와 키 동작(TTL 수정 · 지우기 · 이름 바꾸기 · 삭제).
//
// - string · RedisJSON: 텍스트 · JSON · hex 보기(M3-16 의 셀 상세 `CellDetail` 을 그 꼴의 값으로 다시 쓴다 — JSON 은 원래 글을 지키는
//   트리, hex 는 덤프). string 의 쪽은 바이트 창(`STRING_WINDOW`)이다.
// - hash · set · zset · list · stream: M3-16 의 결과 화면(`ResultView` — 필터 · 정렬 · 복사 · 셀 상세)을 쪽마다 버퍼 하나로 다시 쓴다.
//   list 의 index 는 쪽의 시작 번호(커서)부터 센다. stream 은 항목의 필드마다 한 줄(id 를 되풀이한다).
// - 앞 쪽으로 가려고 지나온 커서를 쌓는다(SCAN 류 커서는 뒤로 갈 수 없다).

import type {
  ColumnMeta,
  DbValue,
  KvKeyInfo,
  KvValue,
  KvValuePage,
  RpcClient,
} from "@pitwall/rpc-client";
import { useEffect, useMemo, useState } from "react";

import { CellDetail } from "../grid/cell-detail";
import { ResultBuffer } from "../grid/result-buffer";
import { ResultView } from "../grid/result-view";
import { useDbGridStrings } from "../grid/strings";
import { formatSize } from "../grid/values";
import type { KeyEntry } from "./key-tree";
import { errorText } from "./kv-gate";
import { type DbRedisStrings, useDbRedisStrings } from "./strings";

/** 컬렉션 쪽 하나의 원소 수(Host 의 기본과 같다). */
export const PAGE_COUNT = 1_000;
/** string 쪽 하나의 바이트 창. */
export const STRING_WINDOW = 1024 * 1024;

export interface KeyActionsApi {
  editTtl(entry: KeyEntry): void;
  clearTtl(entry: KeyEntry): void;
  rename(entry: KeyEntry): void;
  remove(entry: KeyEntry): void;
}

type Load =
  | { kind: "loading" }
  | { kind: "page"; page: KvValuePage }
  | { kind: "failed"; message: string };

/** 타입 이름(보이기용). */
export function typeName(kind: KvKeyInfo["kind"], T: DbRedisStrings["type"]): string {
  return typeof kind === "string" ? T[kind] : kind.other;
}

/** 남은 TTL 의 글(`12 s` · `3 m 20 s` · `2 h 5 m` · `4 d 1 h`). */
export function formatTtl(ms: number): string {
  const s = Math.ceil(ms / 1000);
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} m ${s % 60} s`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h} h ${m % 60} m`;
  return `${Math.floor(h / 24)} d ${h % 24} h`;
}

/** 지금 남은 TTL(ms). 만료가 없으면 `null`. */
export function remainingTtl(entry: KeyEntry, now = performance.now()): number | null {
  const ttl = entry.info.ttlMs;
  return ttl === null ? null : Math.max(0, ttl - (now - entry.at));
}

function TtlText({ entry, T }: { entry: KeyEntry; T: DbRedisStrings["viewer"] }) {
  const [now, setNow] = useState(() => performance.now());
  const ticking = entry.info.ttlMs !== null;
  useEffect(() => {
    if (!ticking) return;
    const timer = setInterval(() => setNow(performance.now()), 1000);
    return () => clearInterval(timer);
  }, [ticking]);
  const left = remainingTtl(entry, now);
  return (
    <span data-testid="redis-ttl">
      {left === null ? T.noTtl : left === 0 ? T.expired : T.ttl(formatTtl(left))}
    </span>
  );
}

const col = (name: string, dbType: string): ColumnMeta => ({ name, dbType, nullable: false });

/** 점수 글을 값으로 — 수면 decimal(숫자처럼 오른쪽 · 수로 정렬), `inf` 등은 글. */
function scoreValue(score: string): DbValue {
  return /^[-+]?(\d+\.?\d*|\.\d+)([eE][-+]?\d+)?$/.test(score)
    ? { decimal: score }
    : { text: score };
}

/** 컬렉션 쪽 하나를 결과 버퍼로. string · JSON · 없는 키는 `undefined`. */
export function pageBuffer(
  value: KvValue,
  start: number,
  T: DbRedisStrings["column"],
): ResultBuffer | undefined {
  if (value === "missing") return undefined;
  const buffer = new ResultBuffer();
  if (value.hash !== undefined) {
    buffer.setColumns([col(T.field, "hash field"), col(T.value, "hash value")]);
    buffer.append(value.hash.map((f) => [f.field, f.value]));
  } else if (value.set !== undefined) {
    buffer.setColumns([col(T.member, "set member")]);
    buffer.append(value.set.map((m) => [m]));
  } else if (value.zset !== undefined) {
    buffer.setColumns([col(T.member, "zset member"), col(T.score, "score")]);
    buffer.append(value.zset.map((m) => [m.member, scoreValue(m.score)]));
  } else if (value.list !== undefined) {
    buffer.setColumns([col(T.index, "index"), col(T.value, "list element")]);
    buffer.append(value.list.map((v, i) => [{ int: start + i }, v]));
  } else if (value.stream !== undefined) {
    buffer.setColumns([
      col(T.id, "stream id"),
      col(T.field, "stream field"),
      col(T.value, "stream value"),
    ]);
    buffer.append(
      value.stream.flatMap((e) =>
        e.fields.length === 0
          ? [[{ text: e.id }, "null", "null"] as DbValue[]]
          : e.fields.map((f) => [{ text: e.id }, f.field, f.value]),
      ),
    );
  } else {
    buffer.dispose();
    return undefined;
  }
  buffer.finish({ kind: "done", truncated: false, elapsedMs: 0, rowsAffected: null });
  return buffer;
}

type StringView = "text" | "json" | "hex";

function utf8Hex(text: string): string {
  let hex = "0x";
  for (const b of new TextEncoder().encode(text)) hex += b.toString(16).padStart(2, "0");
  return hex;
}

/** string · JSON 값을 고른 보기의 꼴로(셀 상세가 그 꼴에 맞는 보기를 그린다). */
function asView(value: DbValue, view: StringView): DbValue {
  if (value === "null") return value;
  const text = value.text ?? value.json;
  if (view === "json" && text !== undefined) return { json: text };
  if (view === "hex" && text !== undefined) {
    const hex = utf8Hex(text);
    return { bytes: { hex, size: (hex.length - 2) / 2, truncated: false } };
  }
  if (view === "text" && value.json !== undefined) return { text: value.json };
  return value;
}

function StringValue({ value, name, json }: { value: DbValue; name: string; json: boolean }) {
  const T = useDbRedisStrings().viewer;
  const G = useDbGridStrings();
  const [view, setView] = useState<StringView>(json ? "json" : "text");
  const shown = useMemo(() => asView(value, view), [value, view]);
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div role="group" className="flex gap-2 border-b border-ui-border px-2 py-1 text-xs">
        {(["text", "json", "hex"] as const).map((v) => (
          <button
            key={v}
            type="button"
            data-view={v}
            aria-pressed={view === v}
            onClick={() => setView(v)}
            className={view === v ? "font-semibold text-ui-accent" : "text-ui-muted-foreground"}
          >
            {T.view[v]}
          </button>
        ))}
      </div>
      <CellDetail value={shown} column={col(name, json ? "json" : "string")} T={G} />
    </div>
  );
}

export function ValueView({
  client,
  sessionId,
  entry,
  actions,
  readOnly,
}: {
  client: RpcClient;
  sessionId: string;
  entry: KeyEntry;
  actions: KeyActionsApi;
  readOnly: boolean;
}) {
  const T = useDbRedisStrings();
  /** 지나온 쪽의 커서(맨 뒤가 지금 쪽). */
  const [cursors, setCursors] = useState<(string | null)[]>([null]);
  const [load, setLoad] = useState<Load>({ kind: "loading" });
  const [round, setRound] = useState(0);
  const cursor = cursors[cursors.length - 1]!;
  const kind = entry.info.kind;
  const count = kind === "string" ? STRING_WINDOW : PAGE_COUNT;

  useEffect(() => {
    const abort = new AbortController();
    setLoad({ kind: "loading" });
    client
      .call(
        "db.kv.get",
        { sessionId, key: entry.key, range: { cursor, count } },
        { signal: abort.signal },
      )
      .then(
        (page) => !abort.signal.aborted && setLoad({ kind: "page", page }),
        (e: unknown) => !abort.signal.aborted && setLoad({ kind: "failed", message: errorText(e) }),
      );
    return () => abort.abort();
  }, [client, sessionId, entry.key, cursor, count, round]);

  const page = load.kind === "page" ? load.page : undefined;
  const start = cursor === null ? 0 : Number.parseInt(cursor, 10) || 0;
  const buffer = useMemo(
    () => (page ? pageBuffer(page.value, start, T.column) : undefined),
    [page, start, T.column],
  );
  useEffect(() => () => buffer?.dispose(), [buffer]);

  const size = entry.info.size;
  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="redis-value" data-key={entry.name}>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-ui-border px-2 py-1 text-xs">
        <span className="font-mono font-semibold break-all select-text">{entry.name}</span>
        <span data-testid="redis-type" className="text-ui-muted-foreground">
          {typeName(kind, T.type)}
        </span>
        <TtlText entry={entry} T={T.viewer} />
        {size !== null && <span>{T.viewer.size(formatSize(size))}</span>}
        {page?.total != null && <span>{T.viewer.total(page.total)}</span>}
        <span className="flex-1" />
        {!readOnly && (
          <>
            <button type="button" data-action="edit-ttl" onClick={() => actions.editTtl(entry)}>
              {T.key.editTtl}
            </button>
            <button
              type="button"
              data-action="clear-ttl"
              disabled={entry.info.ttlMs === null}
              onClick={() => actions.clearTtl(entry)}
              className="disabled:opacity-50"
            >
              {T.key.clearTtl}
            </button>
            <button type="button" data-action="rename" onClick={() => actions.rename(entry)}>
              {T.key.rename}
            </button>
            <button type="button" data-action="delete" onClick={() => actions.remove(entry)}>
              {T.key.delete}
            </button>
          </>
        )}
        <button type="button" data-action="reload" onClick={() => setRound((r) => r + 1)}>
          {T.viewer.reload}
        </button>
      </div>
      {load.kind === "loading" && (
        <p className="p-3 text-ui-muted-foreground">{T.viewer.loading}</p>
      )}
      {load.kind === "failed" && (
        <p role="alert" className="p-3 text-ui-accent">
          {T.viewer.failed(load.message)}
        </p>
      )}
      {page &&
        (page.value === "missing" ? (
          <p data-testid="redis-missing" className="p-3 text-ui-muted-foreground">
            {T.viewer.missing}
          </p>
        ) : page.value.string !== undefined || page.value.json !== undefined ? (
          <StringValue
            key={`${entry.name}\n${cursor ?? ""}`}
            value={(page.value.string ?? page.value.json)!}
            name={entry.name}
            json={page.value.json !== undefined}
          />
        ) : buffer ? (
          <div className="flex min-h-0 flex-1 flex-col">
            <ResultView buffer={buffer} />
          </div>
        ) : (
          <p className="p-3 text-ui-muted-foreground">
            {T.viewer.unsupported(typeName(kind, T.type))}
          </p>
        ))}
      {(cursors.length > 1 || page?.nextCursor != null) && (
        <div className="flex items-center gap-3 border-t border-ui-border px-2 py-1 text-xs">
          <button
            type="button"
            data-action="prev-page"
            disabled={cursors.length <= 1}
            onClick={() => setCursors((c) => c.slice(0, -1))}
            className="disabled:opacity-50"
          >
            {T.viewer.prev}
          </button>
          <span data-testid="redis-page">{T.viewer.page(cursors.length)}</span>
          <button
            type="button"
            data-action="next-page"
            disabled={page?.nextCursor == null}
            onClick={() => {
              const next = page?.nextCursor;
              if (next != null) setCursors((c) => [...c, next]);
            }}
            className="disabled:opacity-50"
          >
            {T.viewer.next}
          </button>
        </div>
      )}
    </div>
  );
}
