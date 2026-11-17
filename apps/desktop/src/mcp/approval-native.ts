// 승인 요청의 OS 알림(16-m5-plan M5-14) — 네이티브(Tauri) 쪽은 src-tauri/src/approval_notice.rs. 시험 · 브라우저에서는 이 인터페이스를
// 갈아 끼운다.
//
// - 명령 `mcp_approval_notify { request, title, body }`: 이 창이 앞에 있으면 아무것도 하지 않는다. 앞에 없으면 OS 알림을 내고
//   (이 창이 다시 앞에 올 때까지는 한 번만 — 요청이 여럿 와도 알림이 쌓이지 않게), 알림을 낼 수 없으면(권한 없음 · 실패) 창의
//   주의 요청(macOS 의 Dock 아이콘 튀기기)으로 대신한다.
// - 이벤트 `mcp-approval-reveal`(payload 는 요청 id): 사람이 그 알림을 눌렀다 — 네이티브가 이 창을 앞으로 가져온 뒤 보낸다. 창은 그
//   요청의 다이얼로그를 다시 연다(Esc 로 닫았던 것이어도).

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

export const REVEAL_EVENT = "mcp-approval-reveal";

export interface ApprovalNotice {
  /** 요청 id. */
  request: number;
  title: string;
  body: string;
}

export interface ApprovalNative {
  notify(notice: ApprovalNotice): void;
  /** 알림을 눌러 이 창이 앞에 왔다. 해제 함수를 돌려준다. */
  onReveal(listener: (request: number) => void): () => void;
}

const LOG = "[mcp]";

export const tauriApprovalNative: ApprovalNative = {
  notify: (notice) => {
    invoke("mcp_approval_notify", { ...notice }).catch((error: unknown) =>
      console.warn(LOG, "could not announce the approval request", error),
    );
  },
  onReveal: (listener) => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    try {
      getCurrentWebviewWindow()
        .listen<number>(REVEAL_EVENT, (e) => listener(e.payload))
        .then((off) => (disposed ? off() : (unlisten = off)))
        .catch((error: unknown) => console.warn(LOG, `${REVEAL_EVENT} listen failed`, error));
    } catch {
      // Tauri 밖(브라우저 · 시험) — 알림이 없다.
    }
    return () => {
      disposed = true;
      unlisten?.();
    };
  },
};
