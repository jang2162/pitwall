// `db.kv.*` 의 관문 답을 화면의 갈래로(14-m3-plan M3-22 ⓑ ⓒ): 확인(`3001` — `data` 는 `DbKvConfirmRequired`)을 받아 다시 부르기,
// 거절 명령(`PERMISSION_DENIED` · `data.reason = "blockedCommand"`), 드라이버 오류(`3003` · `data.error`).
//
// 등급은 Host 가 정한다 — 화면은 분류를 다시 하지 않고, 받은 `3001` 을 M3-19 의 확인 다이얼로그(`KvConfirmQuestion`)로 물은 뒤
// 받은 해시 · 친 이름을 `confirmed` 로 돌려준다. 다시 부른 것이 또 `3001` 이면(그사이 세션의 다른 쓰기가 확인 순번을 올렸다 ·
// 이름이 달랐다) 그 `rejected` 와 함께 다시 묻는다.

import {
  type DbConfirmed,
  type DbKvConfirmRequired,
  ERROR_CODE,
  type JsonValue,
  RpcCallError,
} from "@pitwall/rpc-client";

import { DB_ERROR_CODE } from "../safety/messages";
import type { RiskConfirmAnswer } from "../safety/risk-confirm";
import type { KvBlock } from "./strings";

/** 한 요청을 다시 묻는 횟수의 상한 — 넘으면 마지막 오류를 그대로 던진다. */
const MAX_ASKS = 4;

function dataOf(error: unknown): Record<string, JsonValue> | undefined {
  if (!(error instanceof RpcCallError)) return undefined;
  const data = error.data;
  return data && typeof data === "object" && !Array.isArray(data) ? data : undefined;
}

/** `3001` 이고 `data` 가 Redis 명령의 확인이면 그것. */
export function kvConfirmRequiredOf(error: unknown): DbKvConfirmRequired | undefined {
  if (!(error instanceof RpcCallError) || error.code !== DB_ERROR_CODE.CONFIRM_REQUIRED)
    return undefined;
  const data = dataOf(error);
  if (!data || typeof data.statementsHash !== "string" || !Array.isArray(data.argv))
    return undefined;
  return data as unknown as DbKvConfirmRequired;
}

const BLOCKS: readonly KvBlock[] = ["notAllowed", "blocking", "scanCount"];

/** 거절 명령이면 그 갈래와 명령 이름. */
export function kvBlockedOf(error: unknown): { block: KvBlock; command: string } | undefined {
  if (!(error instanceof RpcCallError) || error.code !== ERROR_CODE.PERMISSION_DENIED)
    return undefined;
  const data = dataOf(error);
  if (data?.reason !== "blockedCommand") return undefined;
  const block = BLOCKS.find((b) => b === data.block);
  if (!block) return undefined;
  return { block, command: typeof data.command === "string" ? data.command : "" };
}

/** `3003` 의 드라이버 오류(`data.error` — `DbError`)면 그 코드 · 글. */
export function kvDbErrorOf(error: unknown): { code: string | null; message: string } | undefined {
  if (!(error instanceof RpcCallError) || error.code !== DB_ERROR_CODE.DB_ERROR) return undefined;
  const inner = dataOf(error)?.error;
  if (!inner || typeof inner !== "object" || Array.isArray(inner)) return undefined;
  return {
    code: typeof inner.code === "string" ? inner.code : null,
    message: typeof inner.message === "string" ? inner.message : error.message,
  };
}

/** 오류의 사람이 읽을 글(Host 의 `invalid params` 는 `data` 글에 까닭이 있다). */
export function errorText(error: unknown): string {
  const db = kvDbErrorOf(error);
  if (db) return db.message;
  if (error instanceof RpcCallError && typeof error.data === "string") return error.data;
  return error instanceof Error ? error.message : String(error);
}

export type Gated<T> = { kind: "done"; value: T } | { kind: "cancelled" } | { kind: "busy" };

/**
 * `call` 을 부르고, `3001` 이면 `ask` 로 물어 받은 확인으로 다시 부른다. 사람이 취소하면 `cancelled`, 다른 질문이 떠 있으면 `busy`.
 * 그 밖의 오류는 그대로 던진다.
 */
export async function withKvConfirm<T>(
  call: (confirmed: DbConfirmed | undefined) => Promise<T>,
  ask: (request: DbKvConfirmRequired) => Promise<RiskConfirmAnswer>,
): Promise<Gated<T>> {
  let confirmed: DbConfirmed | undefined;
  for (let asked = 0; ; asked++) {
    try {
      return { kind: "done", value: await call(confirmed) };
    } catch (e) {
      const request = kvConfirmRequiredOf(e);
      if (!request || asked >= MAX_ASKS) throw e;
      const answer = await ask(request);
      if (answer === undefined) return { kind: "cancelled" };
      if (answer === "busy") return { kind: "busy" };
      confirmed = answer;
    }
  }
}
