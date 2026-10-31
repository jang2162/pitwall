// 고른 셀의 복사 형식(M3-16, 10-ui.md §5.6): ⌘C 는 TSV, ⌘⇧C 는 CSV · JSON · INSERT 문 · TSV 를 고른다.
//
// - TSV: 셀 사이 탭, 행 사이 `\n`, 머리 없이(스프레드시트 붙여넣기용). 탭 · 줄바꿈 · `"` 가 든 값은 스프레드시트처럼 `"…"` 로
//   싸고 안의 `"` 를 두 번 쓴다. NULL 은 빈 칸.
// - CSV: RFC 4180 — 머리 줄(열 이름), 줄 끝 CRLF, `,` · `"` · CR · LF 가 든 필드는 `"…"` 로 싸고 안의 `"` 를 두 번. NULL 은 빈 필드.
// - JSON: 행마다 객체의 배열. 정수(±2^53 안) · 유한한 실수는 수, 큰 정수 · decimal 은 문자열(정밀도), 바이트는 `0x…` 글, json
//   값은 읽을 수 있으면 그 JSON, 아니면 글. 같은 열 이름이 둘이면 뒤의 것에 `_2` …를 붙인다.
// - INSERT: Host 의 `db.sql.insertText`(리터럴 규칙의 정본은 `pw_db_sql::literal`). 잘린 셀이 있으면 Host 에 보내기 전에 거절한다.
//
// 잘린 셀(셀 상한)은 TSV · CSV · JSON 에서 받은 앞부분 그대로다(원래 값이 아니다 — 상태 줄 · 상세 패널이 원래 크기를 보인다).

import {
  type ColumnMeta,
  type DbValue,
  type InsertTextError,
  type ObjectRef,
  RpcCallError,
  type RpcClient,
  type SqlDialect,
} from "@pitwall/rpc-client";

import { isTruncated, valueText } from "./values";

export type CopyFormat = "tsv" | "csv" | "json" | "insert";

export const COPY_FORMATS: readonly CopyFormat[] = ["csv", "json", "insert", "tsv"];

/** 복사할 셀 덩어리: 열(보이는 순서)과 행(보이는 순서)의 값. */
export interface CellBlock {
  columns: ColumnMeta[];
  rows: DbValue[][];
}

function spreadsheetField(v: DbValue): string {
  const t = valueText(v);
  if (t === null) return "";
  return /[\t\r\n"]/.test(t) ? `"${t.replaceAll('"', '""')}"` : t;
}

export function toTsv(block: CellBlock): string {
  return block.rows.map((r) => r.map(spreadsheetField).join("\t")).join("\n");
}

function csvField(t: string | null): string {
  if (t === null) return "";
  return /[",\r\n]/.test(t) ? `"${t.replaceAll('"', '""')}"` : t;
}

export function toCsv(block: CellBlock): string {
  const lines = [block.columns.map((c) => csvField(c.name)).join(",")];
  for (const r of block.rows) lines.push(r.map((v) => csvField(valueText(v))).join(","));
  return `${lines.join("\r\n")}\r\n`;
}

function jsonValue(v: DbValue): unknown {
  if (v === "null") return null;
  if (v.bool !== undefined) return v.bool;
  if (v.int !== undefined) return v.int;
  if (v.float !== undefined) return typeof v.float === "number" ? v.float : v.float;
  if (v.json !== undefined) {
    try {
      return JSON.parse(v.json) as unknown;
    } catch {
      return v.json;
    }
  }
  if (v.array !== undefined) return v.array.map(jsonValue);
  return valueText(v);
}

function uniqueNames(columns: ColumnMeta[]): string[] {
  const seen = new Map<string, number>();
  return columns.map((c) => {
    const n = (seen.get(c.name) ?? 0) + 1;
    seen.set(c.name, n);
    return n === 1 ? c.name : `${c.name}_${n}`;
  });
}

export function toJson(block: CellBlock): string {
  const names = uniqueNames(block.columns);
  const rows = block.rows.map((r) => {
    const o: Record<string, unknown> = {};
    r.forEach((v, i) => {
      o[names[i]!] = jsonValue(v);
    });
    return o;
  });
  return JSON.stringify(rows, null, 2);
}

/** INSERT 로 복사할 수 없는 첫 셀(행 · 열, 덩어리 기준 0부터). */
export function firstTruncatedCell(block: CellBlock): { row: number; column: number } | undefined {
  for (let r = 0; r < block.rows.length; r++) {
    const c = block.rows[r]!.findIndex(isTruncated);
    if (c >= 0) return { row: r, column: c };
  }
  return undefined;
}

export class CopyRefused extends Error {
  constructor(
    readonly reason: "truncated" | "unsupported" | "nul" | "tooLarge",
    readonly cell?: { row: number; column: number },
  ) {
    super(`copy refused: ${reason}`);
  }
}

/** 한 번의 `db.sql.insertText` 에 보내는 셀 수(Host 의 `MAX_INSERT_CELLS` 와 같다). 넘으면 나눠 부른다. */
export const INSERT_CELLS_PER_CALL = 2_000_000;

export async function toInsert(
  block: CellBlock,
  options: { client: RpcClient; dialect: SqlDialect; table: ObjectRef | null },
): Promise<string> {
  const truncated = firstTruncatedCell(block);
  if (truncated) throw new CopyRefused("truncated", truncated);
  const columns = block.columns.map((c) => c.name);
  const perCall = Math.max(1, Math.floor(INSERT_CELLS_PER_CALL / Math.max(1, columns.length)));
  let text = "";
  for (let start = 0; start < block.rows.length; start += perCall) {
    const rows = block.rows.slice(start, start + perCall);
    try {
      const result = await options.client.call("db.sql.insertText", {
        dialect: options.dialect,
        table: options.table,
        columns,
        rows,
      });
      text += result.text;
    } catch (error) {
      const data =
        error instanceof RpcCallError ? (error.data as InsertTextError | undefined) : undefined;
      if (data && typeof data === "object" && "reason" in data) {
        throw new CopyRefused(
          data.reason,
          data.row !== null && data.column !== null
            ? { row: start + data.row, column: data.column }
            : undefined,
        );
      }
      throw error;
    }
  }
  return text;
}

export async function formatBlock(
  format: CopyFormat,
  block: CellBlock,
  insert: { client: RpcClient; dialect: SqlDialect; table: ObjectRef | null } | undefined,
): Promise<string> {
  switch (format) {
    case "tsv":
      return toTsv(block);
    case "csv":
      return toCsv(block);
    case "json":
      return toJson(block);
    case "insert":
      if (!insert) throw new CopyRefused("unsupported");
      return toInsert(block, insert);
  }
}
