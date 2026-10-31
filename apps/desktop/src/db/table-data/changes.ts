// 제출하지 않은 변경 → `db.edit.preview` · `apply` 의 `changes`(14-m3-plan.md M3-12 ⓐ · M3-17). 화면은 DML 을 만들지 않는다 —
// 키 · 열 이름 · 값만 넘기고 문장은 Host(`pw_db_sql::edit`)가 만든다. 키 판정 · 열 이름 맞춤 · 영향 행 규칙의 정본은 Host 다.
//
// - 키는 `db.edit.rowKey` 의 열(메타데이터의 정본 이름)과 그 행의 **읽은 값**(고치기 전)이다. 결과 열 이름에서 키 열을 찾을 때는
//   방언의 규칙(PG 글자 그대로, MySQL · SQLite ASCII 대소문자 무시 — §2 끝의 식별자 문장)으로 맞춘다. 찾지 못하면 편집을 받지
//   않는다(`keyColumnsOf` 가 `undefined`).
// - 열 이름은 결과 열 이름 그대로 보낸다 — Host 가 메타데이터의 열 목록에 방언의 규칙으로 맞춰 정본 이름으로 바꾼다.
// - 사람이 친 글을 값으로: NULL 은 따로(단추), 바이트 열은 `0x…` 면 바이트, 읽은 값이 정수 · 실수이고 글이 그 수면 수, 나머지는
//   글이다 — 서버가 그 열 타입의 입력 함수로 읽는다(PG 텍스트 형식 바인드, MySQL · SQLite 의 형 변환). **decimal 로 보내지 않는다**
//   (M3-12 ⓔ — SQLite 방언의 decimal 은 Host 가 거절하고, 다른 방언에서도 글이 같은 값이다).

import type { ColumnMeta, DbEditChanges, DbValue, SqlDialect } from "@pitwall/rpc-client";

import { isDefaultCell } from "./edit-buffer";
import type { EditBuffer } from "./edit-buffer";

/** 결과 열 이름이 키 열 이름과 같은가(방언의 규칙). */
export function sameColumnName(dialect: SqlDialect, a: string, b: string): boolean {
  if (dialect === "postgres") return a === b;
  // MySQL · SQLite: ASCII 대소문자만 무시한다.
  const lower = (s: string) => s.replace(/[A-Z]/g, (c) => c.toLowerCase());
  return lower(a) === lower(b);
}

/** 키 열마다 결과의 열 번호. 하나라도 없거나 둘 이상이면 `undefined`. */
export function keyColumnsOf(
  dialect: SqlDialect,
  columns: readonly ColumnMeta[],
  key: readonly string[],
): number[] | undefined {
  const out: number[] = [];
  for (const name of key) {
    const found = columns.flatMap((c, i) => (sameColumnName(dialect, c.name, name) ? [i] : []));
    if (found.length !== 1) return undefined;
    out.push(found[0]!);
  }
  return out;
}

/** 값 둘이 같은가(와이어 꼴 그대로). */
export function sameValue(a: DbValue, b: DbValue): boolean {
  return a === b || JSON.stringify(a) === JSON.stringify(b);
}

const SAFE_INT = /^-?\d{1,16}$/;
const FLOAT = /^-?(\d+\.?\d*|\.\d+)([eE][-+]?\d+)?$/;
const HEX = /^0x([0-9a-fA-F]{2})*$/;

/** 바이트 열인가(읽은 값이 바이트거나 타입 이름이). */
function bytesColumn(column: ColumnMeta | undefined, original: DbValue | undefined): boolean {
  if (original !== undefined && original !== "null" && original.bytes !== undefined) return true;
  return /bytea|blob|binary/i.test(column?.dbType ?? "");
}

/** 사람이 친 글 → 보낼 값. */
export function valueFromInput(
  text: string,
  column: ColumnMeta | undefined,
  original: DbValue | undefined,
): DbValue {
  if (bytesColumn(column, original) && HEX.test(text)) {
    const hex = text.toLowerCase();
    return { bytes: { hex, size: (hex.length - 2) / 2, truncated: false } };
  }
  const numeric = original !== undefined && original !== "null" ? original : undefined;
  if (numeric?.int !== undefined && SAFE_INT.test(text)) {
    const n = Number(text);
    if (Number.isSafeInteger(n)) return { int: n };
  }
  if (numeric?.float !== undefined && FLOAT.test(text)) {
    const n = Number(text);
    if (Number.isFinite(n)) return { float: n };
  }
  return { text };
}

/** 셀이 고칠 수 있는 값인가: 잘린 값(원래 값이 아니다)은 고치지 않는다. */
export function editableValue(value: DbValue | undefined): boolean {
  if (value === undefined || value === "null" || isDefaultCell(value)) return true;
  if (value.truncated !== undefined) return false;
  if (value.bytes !== undefined && value.bytes.truncated) return false;
  return true;
}

/** 버퍼의 변경 → `changes`. 키 열을 고친 것은 화면이 막으므로 여기 오지 않는다(오면 Host 가 `keyColumnChanged`). */
export function buildChanges(buffer: EditBuffer, keyColumns: readonly number[]): DbEditChanges {
  const names = buffer.columns.map((c) => c.name);
  const keyOf = (row: number) => {
    const original = buffer.original(row) ?? [];
    return keyColumns.map((c) => ({ column: names[c]!, value: original[c]! }));
  };
  const cells = (values: Map<number, DbValue>) =>
    [...values.entries()]
      .sort(([a], [b]) => a - b)
      .filter(([, v]) => !isDefaultCell(v))
      .map(([c, value]) => ({ column: names[c]!, value }));
  const { updates, inserts, deletes } = buffer.pendingChanges();
  const out: DbEditChanges = {};
  if (updates.length > 0)
    out.updates = updates.map(({ row, values }) => ({ key: keyOf(row), values: cells(values) }));
  if (inserts.length > 0) out.inserts = inserts.map((values) => ({ values: cells(values) }));
  if (deletes.length > 0) out.deletes = deletes.map((row) => ({ key: keyOf(row) }));
  return out;
}
