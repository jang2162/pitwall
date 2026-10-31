// 편집 RPC(`db.edit.preview` · `apply`)의 실패를 사람의 문장으로(14-m3-plan.md M3-12 ⓔ ⓖ · M3-17). 거절의 정본은 Host 다 — 여기는
// Host 가 실어 준 `data` 를 읽기만 한다.
//
// - `CONFLICT` `rowCount`: 영향 행이 1 이 아닌 변경(`data.failure.change` · `rowsAffected`)과 무엇이 되돌려졌는가(`undone`).
// - `CONFLICT` `previewChanged` · `busy`, `3003`(DB 오류 — `data.error.message` 와 `undone`).
// - `INVALID_PARAMS` 의 `DbEditError { change?, column?, reason }`(키 열 수정 · 모르는 열 · 잘린 값 …).
// - 그 밖의 Host 거절(read-only · 미신뢰 …)은 M3-19 의 거절 문장.

import {
  type DbEditChangeRef,
  type DbEditFailure,
  ERROR_CODE,
  type JsonValue,
  RpcCallError,
} from "@pitwall/rpc-client";

import { DB_ERROR_CODE, dbRefusal, dbRefusalText } from "../safety/messages";
import type { DbSafetyStrings } from "../safety/strings";
import type { DbTableDataStrings } from "./strings";

function dataOf(error: unknown): Record<string, JsonValue> | undefined {
  if (!(error instanceof RpcCallError)) return undefined;
  const d = error.data;
  return d && typeof d === "object" && !Array.isArray(d) ? d : undefined;
}

function changeOf(value: JsonValue | undefined): DbEditChangeRef | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  const { kind, index } = value as Record<string, JsonValue>;
  if ((kind === "update" || kind === "insert" || kind === "delete") && typeof index === "number")
    return { kind, index };
  return undefined;
}

/** 실패를 사람의 문장으로. */
export function editFailureText(
  error: unknown,
  T: DbTableDataStrings["fail"],
  S: DbSafetyStrings["refusal"],
): string {
  const refusal = dbRefusal(error);
  if (refusal) return dbRefusalText(refusal, S);
  const data = dataOf(error);
  const label = (change: DbEditChangeRef | undefined) =>
    change ? `${T.change[change.kind](change.index + 1)} — ` : "";
  if (error instanceof RpcCallError && data) {
    const failure = data.failure as unknown as DbEditFailure | undefined;
    const undone =
      failure && typeof failure === "object" && failure.undone in T.undone
        ? ` ${T.undone[failure.undone]}`
        : "";
    if (error.code === ERROR_CODE.CONFLICT) {
      if (data.reason === "rowCount" && failure) {
        const change = changeOf(failure.change as unknown as JsonValue);
        const name = change ? T.change[change.kind](change.index + 1) : "";
        return `${T.rowCount(name, failure.rowsAffected ?? 0)}${undone}`;
      }
      if (data.reason === "previewChanged") return T.changed;
    }
    if (error.code === DB_ERROR_CODE.DB_ERROR) {
      const inner = data.error;
      const message =
        inner &&
        typeof inner === "object" &&
        !Array.isArray(inner) &&
        typeof inner.message === "string"
          ? inner.message
          : error.message;
      const change = changeOf(failure?.change as unknown as JsonValue);
      return `${label(change)}${T.db(message)}${undone}`;
    }
    if (error.code === ERROR_CODE.INVALID_PARAMS && typeof data.reason === "string") {
      const column = typeof data.column === "string" ? ` · ${data.column}` : "";
      return `${label(changeOf(data.change))}${T.invalid(data.reason, column)}`;
    }
  }
  return error instanceof Error ? error.message : String(error);
}
