// 키맵이 네이티브(Tauri)와 주고받는 것: 메뉴 accelerator 동기화, 메뉴에서 고른 액션, 설정 창 열기, Shell 명령 설치, 창 닫기와
// 그 확인.
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
  /**
   * 창이 닫히려 할 때(닫기 버튼 · `closeWindow` · 앱 종료 · 업데이트 뒤 다시 시작) `shouldClose` 를 묻고 `false` 면 닫지 않는다
   * (미저장 확인, M1-13). 닫지 않으면 네이티브에 알린다 — 업데이트 뒤 다시 시작하던 중이면 거둔다(src-tauri/src/update.rs).
   * 해제 함수를 돌려준다. 없으면(시험) 묻지 않는다.
   */
  guardClose?(shouldClose: () => Promise<boolean>): () => void;
  /**
   * 그 프로젝트의 창을 열거나, 이미 열려 있으면 앞으로 가져온다(프로젝트 스위처 ⌘⇧P). 웰컴 창과 같은 명령
   * `open_project_window`(src/welcome/native.ts 머리 주석)다. 없으면(시험) 프로젝트 탭이 없다.
   */
  openProjectWindow?(projectId: string): Promise<void>;
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
  openProjectWindow: (projectId) => invoke("open_project_window", { projectId }),
  closeWindow: () => getCurrentWebviewWindow().close(),
  guardClose(shouldClose) {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    // 듣는 동안 Tauri 는 이 창의 닫기를 웹뷰에 맡긴다. 막지 않으면 핸들러가 끝난 뒤 창을 없앤다(destroy).
    getCurrentWebviewWindow()
      .onCloseRequested(async (event) => {
        if (await shouldClose()) return;
        event.preventDefault();
        invoke("window_close_cancelled").catch((e: unknown) =>
          console.warn("[window] could not report a cancelled close", e),
        );
      })
      .then((off) => (disposed ? off() : (unlisten = off)))
      .catch((e: unknown) => console.warn("[window] close guard failed", e));
    return () => {
      disposed = true;
      unlisten?.();
    };
  },
  ...(/Mac/.test(navigator.userAgent) && {
    installShellCommand: () => invoke<void>("install_shell_command"),
  }),
};
