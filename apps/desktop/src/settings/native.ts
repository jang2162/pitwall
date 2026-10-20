// 설정 창이 네이티브(Tauri)에 맡기는 것: 클립보드 쓰기, Shell 명령 설치. 시험 · 브라우저에서는 이 인터페이스로 갈아 끼운다.

import { invoke } from "@tauri-apps/api/core";

import { tauriKeymapNative } from "../keymap/native";

export interface SettingsNative {
  /** 클립보드에 글자를 쓴다(src-tauri/src/clipboard.rs). */
  copyText(text: string): Promise<void>;
  /** 「Shell 명령 설치」(`cli.install` 과 같은 command). macOS 에만 있다. 결과는 네이티브 대화상자가 알리고, 취소 · 실패면 거부된다. */
  installShellCommand?(): Promise<void>;
}

export const tauriSettingsNative: SettingsNative = {
  copyText: (text) => invoke<void>("write_clipboard", { text }),
  ...(tauriKeymapNative.installShellCommand && {
    installShellCommand: tauriKeymapNative.installShellCommand,
  }),
};
