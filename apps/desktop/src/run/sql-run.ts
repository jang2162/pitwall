// 실행 구성의 `sql` 종류를 사람이 창에서 돌린다(17-m6-plan M6-09 ③ — 메인 판정 M6-P16). 그 클릭이 D48 의 「사람이 누름」 이다.
// `run.sql.start` 를 부르고, Host 가 `3001`(`data` 는 `RunSqlConfirmRequired`)을 주면 **SQL 콘솔의 위험 확인을 같은 부품으로**
// 묻는다(db/safety/risk-confirm.tsx — 등급 · 문장 · 연결 표지, 프로덕션 연결의 파괴 등급이면 연결 이름 치기). 받은 확인을
// `confirmed` 로 실어 다시 부른다. 확인은 Host 가 그때 읽은 파일 내용의 해시에 묶여 있어, 그사이 파일이 바뀌었으면 다시 `3001`
// (`rejected: "hashMismatch"`)이 오고 새 내용으로 다시 묻는다.
//
// 등급은 Host 가 정한다 — 화면은 파일을 읽거나 분류하지 않는다. 실행 선택기 · 콘솔(M6-06 · M6-07)이 `sql` 구성을 고르면 `run.start`
// 대신 이것을 부른다(`run.start` 는 `sql` 구성을 띄우지 않는다).

import {
  type DbConfirmed,
  type JsonValue,
  type RunInfo,
  type RunSqlConfirmRequired,
  type RunSqlStartParams,
  RpcCallError,
} from "@pitwall/rpc-client";

import { DB_ERROR_CODE } from "../db/safety/messages";
import type { RiskConfirmAnswer, RiskConfirmQuestion } from "../db/safety/risk-confirm";

/** 한 실행을 다시 묻는 횟수의 상한 — 넘으면 마지막 오류를 그대로 던진다(Redis 콘솔의 `withKvConfirm` 과 같다). */
const MAX_ASKS = 4;

/** `3001` 이고 `data` 가 `sql` 실행 구성의 확인이면 그것. */
export function sqlRunConfirmOf(error: unknown): RunSqlConfirmRequired | undefined {
  if (!(error instanceof RpcCallError) || error.code !== DB_ERROR_CODE.CONFIRM_REQUIRED)
    return undefined;
  const data: JsonValue | undefined = error.data;
  if (!data || typeof data !== "object" || Array.isArray(data)) return undefined;
  const request = data.request;
  if (!request || typeof request !== "object" || Array.isArray(request)) return undefined;
  if (typeof request.statementsHash !== "string" || typeof data.sql !== "string") return undefined;
  return data as unknown as RunSqlConfirmRequired;
}

/** 확인 다이얼로그(SQL 콘솔과 같은 부품)에 넘길 질문 — 문장 범위는 `sql`(파일들을 이은 글) 기준이다. */
export function sqlRunQuestion(data: RunSqlConfirmRequired): RiskConfirmQuestion {
  return {
    request: data.request,
    connection: {
      name: data.connection.name,
      production: data.connection.production,
      readOnly: data.connection.readOnly,
    },
    sql: data.sql,
  };
}

export type SqlRunStarted =
  | { kind: "started"; run: RunInfo }
  | { kind: "cancelled" }
  | { kind: "busy" };

/**
 * `sql` 구성을 돌린다: `call` 로 `run.sql.start` 를 부르고, `3001` 이면 `confirm` 으로 물어 받은 확인으로 다시 부른다. 사람이
 * 취소하면 `cancelled`, 다른 확인이 떠 있으면 `busy`. 그 밖의 오류(미신뢰 · read-only 연결의 쓰기 · 비밀 필요 등)는 그대로 던진다.
 */
export async function startSqlRun(
  call: (params: RunSqlStartParams) => Promise<RunInfo>,
  params: Omit<RunSqlStartParams, "confirmed">,
  confirm: (question: RiskConfirmQuestion) => Promise<RiskConfirmAnswer>,
): Promise<SqlRunStarted> {
  let confirmed: DbConfirmed | undefined;
  for (let asked = 0; ; asked++) {
    try {
      return { kind: "started", run: await call({ ...params, confirmed: confirmed ?? null }) };
    } catch (e) {
      const data = sqlRunConfirmOf(e);
      if (!data || asked >= MAX_ASKS) throw e;
      const answer = await confirm(sqlRunQuestion(data));
      if (answer === undefined) return { kind: "cancelled" };
      if (answer === "busy") return { kind: "busy" };
      confirmed = answer;
    }
  }
}
