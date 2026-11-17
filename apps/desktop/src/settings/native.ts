// 설정 창이 네이티브(Tauri)에 맡기는 것: 클립보드 쓰기, Shell 명령 설치, 설정 파일을 단일 파일 창으로 열기(설정 > MCP 의
// 「파일로 열기」 — 16-m5-plan M5-15). 시험 · 브라우저에서는 이 인터페이스로 갈아 끼운다.

import { invoke } from "@tauri-apps/api/core";

import { tauriKeymapNative } from "../keymap/native";

export interface SettingsNative {
  /** 클립보드에 글자를 쓴다(src-tauri/src/clipboard.rs). */
  copyText(text: string): Promise<void>;
  /** 「Shell 명령 설치」(`cli.install` 과 같은 command). macOS 에만 있다. 결과는 네이티브 대화상자가 알리고, 취소 · 실패면 거부된다. */
  installShellCommand?(): Promise<void>;
  /**
   * 설정 파일을 단일 파일 창으로 연다(명령 `open_file_window`, src-tauri/src/windows.rs). 경로는 Host 가 준 것만 넘긴다 — 화면이
   * 조립하지 않는다(설정 > MCP 의 전역 settings.toml · 고른 프로젝트의 설정 파일). 시험에서는 없을 수 있다.
   */
  openFile?(path: string): Promise<void>;
}

export const tauriSettingsNative: SettingsNative = {
  copyText: (text) => invoke<void>("write_clipboard", { text }),
  openFile: (path) => invoke<void>("open_file_window", { path, line: null, col: null }),
  ...(tauriKeymapNative.installShellCommand && {
    installShellCommand: tauriKeymapNative.installShellCommand,
  }),
};
