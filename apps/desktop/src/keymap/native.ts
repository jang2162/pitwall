// 키맵이 네이티브(Tauri)와 주고받는 것: 메뉴 accelerator 동기화, 메뉴에서 고른 액션, 설정 창 열기, Shell 명령 설치.
// 시험 · 브라우저에서는 Tauri 가 없으므로 이 인터페이스로 갈아 끼운다.

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

/** src-tauri/src/menu.rs 의 `MENU_ACTION_EVENT`. 메뉴 항목을 고르면 포커스된 창에 액션 id 를 보낸다. */
export const MENU_ACTION_EVENT = "keymap://run-action";

export interface KeymapNative {
  /** 액션 id → accelerator(없으면 `null`). 메뉴에 없는 액션은 네이티브가 무시한다. */
  syncMenu(accelerators: Record<string, string | null>): Promise<void>;
  /** 네이티브 메뉴에서 고른 액션. 해제 함수를 돌려준다. */
  onMenuAction(listener: (action: string) => void): () => void;
  openSettings(): Promise<void>;
  /** 이 창을 닫는다(`window.close`). 창의 닫기 버튼과 같은 길(close requested)을 지난다. */
  closeWindow(): Promise<void>;
  /** 「Shell 명령 설치」(F2-1). macOS 에만 있다. 결과는 네이티브 대화상자가 알린다(src-tauri/src/shell_command.rs). */
  installShellCommand?(): Promise<void>;
}

export const tauriKeymapNative: KeymapNative = {
  syncMenu: (accelerators) => invoke("keymap_sync_menu", { accelerators }),
  onMenuAction(listener) {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    // 이 창을 대상으로 보낸 것만 받는다 — 전역 listen 은 다른 창으로 간 것도 받는다.
    getCurrentWebviewWindow()
      .listen<string>(MENU_ACTION_EVENT, (e) => listener(e.payload))
      .then((off) => (disposed ? off() : (unlisten = off)))
      .catch((e: unknown) => console.warn("[keymap] menu listen failed", e));
    return () => {
      disposed = true;
      unlisten?.();
    };
  },
  openSettings: () => invoke("open_settings_window"),
  closeWindow: () => getCurrentWebviewWindow().close(),
  ...(/Mac/.test(navigator.userAgent) && {
    installShellCommand: () => invoke<void>("install_shell_command"),
  }),
};
