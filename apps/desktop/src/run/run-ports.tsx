// 실행이 듣는 포트의 자리(17-m6-plan M6-08 — 「실행」 툴 윈도우의 탭 머리 · 상태바 `run` 항목에 끼운다). 자리와 렌더 함수만 이
// 행(M6-06)이 두고, 내용(`RunInfo` 의 포트 칸 · `run.ports` 이벤트 · 누르면 `http://localhost:<port>` 열기)은 M6-08 이 채운다.
// 그릴 것이 없으면 `null` 이다 — 그때는 자리도 차지하지 않는다.

import type { RunInfo } from "@pitwall/rpc-client";
import type { ReactNode } from "react";

/** 탭 머리의 포트(예: `:5173`). */
export function RunTabPorts(_props: { run: RunInfo }): ReactNode {
  return null;
}

/** 상태바 `run` 항목 옆의 포트. */
export function RunStatusPorts(_props: { runs: readonly RunInfo[] }): ReactNode {
  return null;
}
