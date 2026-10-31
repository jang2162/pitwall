// Host 오류 · 세션 알림을 사람의 문장으로(14-m3-plan.md M3-19 ③, M3-05 행 ⓒ ⓓ).
//
// - 거절 문장: Host 가 돌리지 않은 까닭(`PERMISSION_DENIED` · `CONFLICT` · `INVALID_PARAMS` 의 `data.reason`, crates/pw-db 의
//   gate · execute · session · trust). read-only 연결이 거절한 문장 목록(`data.statements`)이 있으면 함께 돌려준다.
// - `3001`(확인 필요)의 `data` 를 꺼낸다 — 위험 쿼리 확인(risk-confirm.tsx)에 그대로 넘긴다.
// - 세션 알림(`DbSessionNotice`)의 문장 — 콘솔 탭(M3-15)이 알림 · 위 막대에 보인다.

import {
  type DbConfirmRequired,
  type DbSessionNotice,
  type DbStatementClass,
  ERROR_CODE,
  type JsonValue,
  RpcCallError,
} from "@pitwall/rpc-client";

import { type DbRefusalReason, type DbSafetyStrings, useDbSafetyStrings } from "./strings";

/** DB RPC 의 오류 코드(crates/pw-db/src/gate.rs `code`, trust.rs `code`). 생성 타입의 `ERROR_CODE` 에 없는 앱 코드다. */
export const DB_ERROR_CODE = {
  /** 확인이 필요하다 — `data` 는 `DbConfirmRequired`. */
  CONFIRM_REQUIRED: 3001,
  /** 이 취소는 트랜잭션 전체를 되돌린다 — `db.query.cancel { confirmRollback: true }` 로 다시 부른다. */
  CANCEL_ROLLS_BACK: 3002,
  /** DB 가 거절했다 — `data.error` 는 `DbError`. */
  DB_ERROR: 3003,
  /** 비밀 필요(M3-02). */
  SECRET_REQUIRED: 3101,
  /** 확인 필요(M3-02 — `env` 참조 · 루트 밖 SQLite). */
  APPROVAL_REQUIRED: 3102,
} as const;

/** 거절 까닭마다 그것을 내는 오류 코드. 코드가 다르면 같은 이름이라도 거절로 읽지 않는다. */
const REFUSAL_CODES: Record<DbRefusalReason, number> = {
  readOnlyConnection: ERROR_CODE.PERMISSION_DENIED,
  untrusted: ERROR_CODE.PERMISSION_DENIED,
  notProjectWindow: ERROR_CODE.PERMISSION_DENIED,
  notAQuery: ERROR_CODE.INVALID_PARAMS,
  transactionStatement: ERROR_CODE.INVALID_PARAMS,
  busy: ERROR_CODE.CONFLICT,
  configChanged: ERROR_CODE.CONFLICT,
  transactionOpen: ERROR_CODE.CONFLICT,
};

export interface DbRefusal {
  reason: DbRefusalReason;
  /** Host 가 준 까닭 글(`transactionStatement` 의 `data.message`). */
  message: string;
  /** 거절한 문장(read-only 연결 · `db.query.page` 가 싣는다). */
  statements: DbStatementClass[];
}

function dataOf(error: unknown): Record<string, JsonValue> | undefined {
  if (!(error instanceof RpcCallError)) return undefined;
  const data = error.data;
  return data && typeof data === "object" && !Array.isArray(data) ? data : undefined;
}

/** 오류가 Host 의 DB 거절이면 그 까닭. 아니면 `undefined`(보통 오류로 다룬다). */
export function dbRefusal(error: unknown): DbRefusal | undefined {
  const data = dataOf(error);
  if (!data || !(error instanceof RpcCallError)) return undefined;
  const reason = data.reason;
  if (typeof reason !== "string" || !Object.hasOwn(REFUSAL_CODES, reason)) return undefined;
  const known = reason as DbRefusalReason;
  if (REFUSAL_CODES[known] !== error.code) return undefined;
  return {
    reason: known,
    message: typeof data.message === "string" ? data.message : "",
    statements: Array.isArray(data.statements)
      ? (data.statements as unknown as DbStatementClass[])
      : [],
  };
}

/** 거절의 문장. */
export function dbRefusalText(refusal: DbRefusal, T: DbSafetyStrings["refusal"]): string {
  return T[refusal.reason](refusal.message);
}

/** `3001` 이면 그 `data`(확인할 것). 아니면 `undefined`. */
export function confirmRequiredOf(error: unknown): DbConfirmRequired | undefined {
  if (!(error instanceof RpcCallError) || error.code !== DB_ERROR_CODE.CONFIRM_REQUIRED)
    return undefined;
  const data = dataOf(error);
  if (!data || typeof data.statementsHash !== "string" || !Array.isArray(data.statements))
    return undefined;
  return data as unknown as DbConfirmRequired;
}

/** 거절 문장 한 줄(`role="alert"`). 거절이 아닌 오류면 아무것도 그리지 않는다 — 부르는 쪽이 보통 오류로 보인다. */
export function DbRefusalMessage({ error }: { error: unknown }) {
  const T = useDbSafetyStrings().refusal;
  const refusal = dbRefusal(error);
  if (!refusal) return null;
  return (
    <p
      role="alert"
      data-testid="db-refusal"
      data-reason={refusal.reason}
      className="text-ui-accent"
    >
      {dbRefusalText(refusal, T)}
    </p>
  );
}

/** 세션 알림의 문장. `formatTime` 은 `unattended` 의 시각(유닉스 밀리초)을 사람의 시각으로 바꾼다. */
export function dbNoticeText(
  notice: DbSessionNotice,
  T: DbSafetyStrings["notice"],
  formatTime: (ms: number) => string = (ms) => new Date(ms).toLocaleTimeString(),
): string {
  switch (notice.kind) {
    case "rolledBack":
      return T.rolledBack(notice.message);
    case "committedImplicitly":
      return T.committedImplicitly;
    case "transactionEnded":
      return notice.message ? T.transactionEndedMessage(notice.message) : T.transactionEnded;
    case "transactionUnknown":
      return T.transactionUnknown(notice.message);
    case "transactionOpened":
      return T.transactionOpened;
    case "connectionLost":
      if (notice.outcomeUnknown) return T.connectionLostOutcomeUnknown;
      return notice.hadTransaction ? T.connectionLostRolledBack : T.connectionLost;
    case "configChanged":
      return T.configChanged;
    case "unattended":
      return T.unattended(formatTime(notice.until));
    case "unattendedRolledBack":
      return T.unattendedRolledBack;
  }
}
