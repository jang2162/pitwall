// 런타임 환경(03 §5-1): 「환경 다시 읽기」 액션. 로그인 셸 설정을 바꾼 뒤 앱을 다시 띄우지 않고 새 환경을 받는다 — Host 가
// 캐시를 잊고 `runtime.changed` 를 낸다(runtime.reload, crates/pw-runtime). 이미 연 터미널의 환경은 바뀌지 않는다.
// 프로젝트 창에만 등록하므로 문자열은 프로젝트 창 문자열(layout/strings.ts, project.ftl)에 있다.

import type { Action } from "@pitwall/keymap";
import { useMemo, useRef } from "react";

import { type ProjectStrings, useProjectStrings } from "../layout/strings";
import { type NotificationInput, useNotify } from "../notifications/notifications";
import { useHost } from "../rpc/host";

export function runtimeActions(deps: {
  strings: ProjectStrings["runtime"];
  reload(): Promise<void>;
  /** 실패를 알린다(오류 → 토스트 + 알림 툴 윈도우). 본문은 오류 원문이다. */
  notify(input: NotificationInput): void;
}): Action[] {
  const T = deps.strings;
  return [
    {
      id: "runtime.reload",
      label: T.reload,
      keywords: ["Reload Runtime Environment", "Login Shell", "PATH", "환경"],
      run: () =>
        deps.reload().then(
          () => console.info("[runtime] runtime environment reloaded"),
          (error: unknown) =>
            deps.notify({
              level: "error",
              title: T.reloadFailed,
              body: error instanceof Error ? error.message : String(error),
            }),
        ),
    },
  ];
}

/** 프로젝트 창의 런타임 액션. `NotificationProvider` 안에서 부른다. */
export function useRuntimeActions(projectId: string): Action[] {
  const strings = useProjectStrings().runtime;
  const notify = useNotify();
  const { client } = useHost();
  const latest = useRef(client);
  latest.current = client;
  return useMemo(
    () =>
      runtimeActions({
        strings,
        notify,
        reload: async () => {
          const client = latest.current;
          if (!client) throw new Error("not connected to the host");
          await client.call("runtime.reload", { projectId });
        },
      }),
    [strings, notify, projectId],
  );
}
