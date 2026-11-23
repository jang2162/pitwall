// 첫 실행 확인(17-m6-plan M6-07 ⑥ — 머리 규칙은 crates/pw-run/src/runs/confirm.rs): 창의 `run.start` · `run.restart` 는 띄울 내용이
// 「사람이 본 구성」 이 아니면 띄우지 않고 `7001`(`data` 는 `RunConfirmRequired`)로 답한다. 창은 그 내용을 보이고(run-confirm-dialog.tsx)
// [실행] 을 누르면 받은 `hash` 를 `confirmed` 로 실어 다시 부른다. 그사이 파일이 바뀌었으면 Host 가 새 내용으로 다시 `7001` 을 낸다.
//
// 묻는 쪽(대화상자)은 창에 하나다 — 실행 선택기 · `⌃R` · 「실행」 툴 윈도우의 다시 실행이 같은 것을 쓴다(`setRunConfirmAsker`).

import {
  type DbConfirmed,
  type JsonValue,
  RpcCallError,
  type RunConfirmRequired,
  type RunSqlConfirmRequired,
} from "@pitwall/rpc-client";

import { sqlRunConfirmOf } from "./sql-run";

/** `pw_run::runs::confirm::code`. */
export const RUN_ERROR_CODE = { CONFIRM_REQUIRED: 7001 } as const;

/** 한 실행을 다시 묻는 횟수의 상한 — 넘으면 마지막 오류를 그대로 던진다(sql-run.ts 와 같다). */
const MAX_ASKS = 4;

/** `7001` 이고 `data` 가 첫 실행 확인이면 그것. */
export function runConfirmOf(error: unknown): RunConfirmRequired | undefined {
  if (!(error instanceof RpcCallError) || error.code !== RUN_ERROR_CODE.CONFIRM_REQUIRED)
    return undefined;
  const data: JsonValue | undefined = error.data;
  if (!data || typeof data !== "object" || Array.isArray(data)) return undefined;
  if (typeof data.hash !== "string" || !data.shown || typeof data.shown !== "object")
    return undefined;
  return data as unknown as RunConfirmRequired;
}

/** 사람에게 묻는다 — [실행] 이면 참. */
export type RunConfirmAsker = (data: RunConfirmRequired) => Promise<boolean>;

/** 창의 묻는 쪽 — 그 실행이 도는 머신을 함께 받아 문장에 넣는다(18-m7-plan M7-14 리뷰 B2). */
export type MachineConfirmAsker = (data: RunConfirmRequired, machine: string) => Promise<boolean>;

let asker: MachineConfirmAsker | undefined;

/** 이 창의 묻는 쪽을 둔다(대화상자를 그리는 컴포넌트가 붙어 있는 동안). 걷는 함수를 돌려준다. */
export function setRunConfirmAsker(next: MachineConfirmAsker): () => void {
  asker = next;
  return () => {
    if (asker === next) asker = undefined;
  };
}

/**
 * 그 머신에서 도는 실행의 묻는 쪽. 확인 창의 머신은 **그 실행이 실제로 도는 머신**이다 — 실행 선택기의 대상이 아니라 다시 실행하는
 * 탭의 머신 등. 묻는 쪽이 없으면 `undefined`(띄우지 않는 쪽으로).
 */
export function confirmAskerOn(machine: string): RunConfirmAsker | undefined {
  const current = asker;
  return current && ((data) => current(data, machine));
}

/**
 * `call(confirmed)` 로 띄우고, 첫 실행 확인이면 물어 받은 `hash` 로 다시 부른다. 사람이 취소하면 `undefined`. 묻는 쪽이 없으면 묻지
 * 않고 그 오류를 던진다(띄우지 않는 쪽으로).
 */
export async function withRunConfirm<T>(
  call: (confirmed: string | null) => Promise<T>,
  ask: RunConfirmAsker | undefined = confirmAskerOn("local"),
): Promise<T | undefined> {
  let confirmed: string | null = null;
  for (let asked = 0; ; asked++) {
    try {
      return await call(confirmed);
    } catch (e) {
      const data = runConfirmOf(e);
      if (!data || !ask || asked >= MAX_ASKS) throw e;
      if (!(await ask(data))) return undefined;
      confirmed = data.hash;
    }
  }
}

/** 사슬의 `sql` 단계 위험 확인에 답한다(sql-run.ts 와 같은 다이얼로그). 취소 · 다른 확인이 떠 있으면 `undefined`. */
export type SqlStepAsker = (data: RunSqlConfirmRequired) => Promise<DbConfirmed | undefined>;

/**
 * 사슬(`before` · 복합 구성 — 단계가 여럿이라 확인도 여럿)을 다시 묻는 횟수의 상한: 사슬의 단계 상한(32 —
 * `pw_run::config::graph::MAX_STEPS`)의 `sql` 확인 모두와 첫 실행 확인, 그리고 그사이 바뀐 내용으로 한 번 더.
 */
const MAX_CHAIN_ASKS = 32 + 2;

/**
 * 사슬도 띄우는 `run.start`(17-m6-plan M6-21 — crates/pw-run/src/runs/chain.rs 「창의 길」): Host 는 모든 확인을 첫 단계를 띄우기
 * 전에 받는다 — 첫 실행 확인(`7001`, 모든 단계)과 `sql` 단계마다의 위험 확인(`3001`, `data.config` 가 그 단계). 받은 것을 모두 실어
 * 다시 부른다(앞에서 받은 확인은 Host 가 아직 쓰지 않았다). 사람이 하나라도 취소하면 `undefined` — 아무것도 돌지 않았다.
 */
export async function withRunConfirms<T>(
  call: (confirmed: string | null, sqlConfirmed: Record<string, DbConfirmed> | null) => Promise<T>,
  askSql: SqlStepAsker | undefined,
  ask: RunConfirmAsker | undefined = confirmAskerOn("local"),
): Promise<T | undefined> {
  let confirmed: string | null = null;
  const sqlConfirmed: Record<string, DbConfirmed> = {};
  for (let asked = 0; ; asked++) {
    try {
      return await call(
        confirmed,
        Object.keys(sqlConfirmed).length > 0 ? { ...sqlConfirmed } : null,
      );
    } catch (e) {
      if (asked >= MAX_CHAIN_ASKS) throw e;
      const run = runConfirmOf(e);
      if (run) {
        if (!ask) throw e;
        if (!(await ask(run))) return undefined;
        confirmed = run.hash;
        continue;
      }
      const sql = sqlRunConfirmOf(e);
      const config = (e instanceof RpcCallError ? e.data : undefined) as
        | { config?: unknown }
        | undefined;
      if (!sql || typeof config?.config !== "string" || !askSql) throw e;
      const answer = await askSql(sql);
      if (answer === undefined) return undefined;
      sqlConfirmed[config.config] = answer;
    }
  }
}
