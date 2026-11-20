// 디버그 어댑터가 없을 때 설치 확인으로 잇는다(17-m6-plan 「6. 실기 확인 시나리오」 8 — 「⌃D → 어댑터 설치 확인 창(출처 · 라이선스)
// → 그 줄에서 멈춤」, 실기 F-1). Host 의 `debug.start` 는 어댑터를 찾지 못하면 invalid params 에 `data.reason = "adapterMissing"` 과
// 그 해석(`data.resolution` — `notInstalled` 면 설치 패키지 `mason`)을 싣는다(crates/pw-debug/src/adapter.rs `spec`).
//
// - 설치는 설정 > 도구 · 언어 서버 배너와 같은 길이다: 설치기의 확인 흐름(src/tools/install.ts `changeTool` — `4001` 이면 확인 창,
//   [설치] 를 누른 때만 `confirmed` 를 실어 다시 부른다)과 같은 확인 창(src/tools/confirm-dialog.tsx).
// - 설치가 끝나면 같은 시작을 한 번 더 부른다(사람이 다시 누르지 않아도). 확인 창을 취소하면 시작하지 않는다.
// - 설치 패키지가 없는 해석(경로 설정이 잘못됨 등)은 설치로 고칠 수 없어 그 오류를 그대로 던진다.
// - 사람의 🐞 · ⌃D 만 이 길을 지난다. 에이전트에게는 디버그를 여는 툴도 설치하는 툴도 없다(mcp_tests/debug.rs).

import { ERROR_CODE, type JsonValue, RpcCallError } from "@pitwall/rpc-client";

export interface MissingAdapter {
  /** 어댑터 id(`js-debug`). */
  adapter: string;
  /** 설치할 Mason 패키지(`js-debug-adapter`). */
  mason: string;
}

const isObject = (v: JsonValue | undefined): v is { [key: string]: JsonValue } =>
  typeof v === "object" && v !== null && !Array.isArray(v);

/** 설치로 고칠 수 있는 「어댑터 없음」 이면 그 어댑터와 패키지. */
export function missingAdapterOf(error: unknown): MissingAdapter | undefined {
  if (!(error instanceof RpcCallError) || error.code !== ERROR_CODE.INVALID_PARAMS)
    return undefined;
  const data = error.data;
  if (!isObject(data) || data.reason !== "adapterMissing") return undefined;
  const resolution = data.resolution;
  if (!isObject(resolution) || resolution.kind !== "notInstalled") return undefined;
  if (typeof resolution.mason !== "string" || resolution.mason === "") return undefined;
  return {
    adapter: typeof data.adapter === "string" ? data.adapter : "",
    mason: resolution.mason,
  };
}

/**
 * `start` 를 부르고, 설치로 고칠 수 있는 「어댑터 없음」 이면 `install` 로 설치한 뒤 한 번 더 부른다. `install` 이 거짓(사람이
 * 취소)이면 `undefined` — 시작하지 않는다. 설치 뒤에도 실패하면 그 오류를 던진다(다시 설치를 묻지 않는다).
 */
export async function startOrInstallAdapter<T>(
  start: () => Promise<T>,
  install: (missing: MissingAdapter) => Promise<boolean>,
): Promise<T | undefined> {
  try {
    return await start();
  } catch (e) {
    const missing = missingAdapterOf(e);
    if (!missing) throw e;
    if (!(await install(missing))) return undefined;
    return await start();
  }
}
