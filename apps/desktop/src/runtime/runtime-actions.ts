// 런타임 환경(03 §5-1): 「환경 다시 읽기」 액션. 로그인 셸 설정을 바꾼 뒤 앱을 다시 띄우지 않고 새 환경을 받는다 — Host 가
// 캐시를 잊고 `runtime.changed` 를 낸다(runtime.reload, crates/pw-runtime). 이미 연 터미널의 환경은 바뀌지 않는다.

import type { Action } from "@pitwall/keymap";
import { useMemo, useRef } from "react";

import { type AppStrings, useAppStrings } from "../i18n/app-strings";
import { useHost } from "../rpc/host";

const LOG = "[runtime]";

export function runtimeActions(deps: {
  strings: AppStrings["actions"];
  /** 실패는 로그로 남긴다(상태바 표시는 런타임 상태 표시와 함께). */
  reload(): Promise<void>;
}): Action[] {
  return [
    {
      id: "runtime.reload",
      label: deps.strings.runtimeReload,
      keywords: ["Reload Runtime Environment", "Login Shell", "PATH", "환경"],
      run: () =>
        deps.reload().then(
          () => console.info(LOG, "runtime environment reloaded"),
          (e: unknown) => console.warn(LOG, "runtime.reload failed", e),
        ),
    },
  ];
}

/** 프로젝트 창의 런타임 액션. */
export function useRuntimeActions(projectId: string): Action[] {
  const strings = useAppStrings().actions;
  const { client } = useHost();
  const latest = useRef(client);
  latest.current = client;
  return useMemo(
    () =>
      runtimeActions({
        strings,
        reload: async () => {
          const client = latest.current;
          if (!client) throw new Error("not connected to the host");
          await client.call("runtime.reload", { projectId });
        },
      }),
    [strings, projectId],
  );
}
