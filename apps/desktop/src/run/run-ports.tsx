// 실행이 듣는 포트의 칩 자리(17-m6-plan M6-08 — 「실행」 툴 윈도우의 탭 머리 · 상태바 `run` 항목). Host 쪽 포트(`RunInfo.ports` ·
// `run.ports` 이벤트)는 M6-08 이 내고, 칩(누르면 `http://localhost:<port>` — IPv6 에만 듣으면 `[::1]` — 열기, `0.0.0.0` · `::` 에
// 듣는 포트는 「모든 주소」 표지)은 M6-08 이 병합된 뒤 이 행(M6-06)이 여기를 채운다(코디네이터 판정 2026-11-18). 칩의 문구
// (`runStrings().port`)는 이미 run.ftl 에 있다. 그릴 것이 없으면 `null` 이다 — 그때는 자리도 차지하지 않는다.

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
