// 디버그 세션이 멈췄을 때의 창 주의 요청(17-m6-plan M6-18 ④) — 네이티브(Tauri) 쪽은 src-tauri/src/debug_attention.rs. 승인 알림
// (src/mcp/approval-native.ts)과 같은 길: 명령 `debug_stopped_attention` 은 이 창이 앞에 없을 때만 주의를 청한다(macOS 의 Dock 아이콘
// 튀기기). 시험 · 브라우저에서는 이 함수를 갈아 끼운다.

import { invoke } from "@tauri-apps/api/core";

export function tauriDebugAttention(): void {
  invoke("debug_stopped_attention").catch((error: unknown) =>
    console.warn("[debug] could not ask for attention", error),
  );
}
