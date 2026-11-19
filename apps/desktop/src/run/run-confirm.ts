// 첫 실행 확인(17-m6-plan M6-07 ⑥ — 머리 규칙은 crates/pw-run/src/runs/confirm.rs): 창의 `run.start` · `run.restart` 는 띄울 내용이
// 「사람이 본 구성」 이 아니면 띄우지 않고 `7001`(`data` 는 `RunConfirmRequired`)로 답한다. 창은 그 내용을 보이고(run-confirm-dialog.tsx)
// [실행] 을 누르면 받은 `hash` 를 `confirmed` 로 실어 다시 부른다. 그사이 파일이 바뀌었으면 Host 가 새 내용으로 다시 `7001` 을 낸다.
//
// 묻는 쪽(대화상자)은 창에 하나다 — 실행 선택기 · `⌃R` · 「실행」 툴 윈도우의 다시 실행이 같은 것을 쓴다(`setRunConfirmAsker`).

import { type JsonValue, RpcCallError, type RunConfirmRequired } from "@pitwall/rpc-client";

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

let asker: RunConfirmAsker | undefined;

/** 이 창의 묻는 쪽을 둔다(대화상자를 그리는 컴포넌트가 붙어 있는 동안). 걷는 함수를 돌려준다. */
export function setRunConfirmAsker(next: RunConfirmAsker): () => void {
  asker = next;
  return () => {
    if (asker === next) asker = undefined;
  };
}

/**
 * `call(confirmed)` 로 띄우고, 첫 실행 확인이면 물어 받은 `hash` 로 다시 부른다. 사람이 취소하면 `undefined`. 묻는 쪽이 없으면 묻지
 * 않고 그 오류를 던진다(띄우지 않는 쪽으로).
 */
export async function withRunConfirm<T>(
  call: (confirmed: string | null) => Promise<T>,
  ask: RunConfirmAsker | undefined = asker,
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
