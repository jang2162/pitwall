// 디버그 키맵 액션(17-m6-plan M6-18 ③ — 키맵 액션 · `debugging` 문맥의 주인은 이 행이다, 17 「공유 부품의 주인」). 키는
// extensions/keymap-default/keymaps/default.toml 의 「디버그」 묶음이고, 스텝 · 계속 · 식 평가는 `debugging` 문맥에서만 키를 받는다.
// 액션은 문맥과 따로 스스로도 켜짐을 가른다(멈춰 있을 때만 스텝) — 액션 찾기 · 메뉴에서 불러도 같다.
//
// 실행 / 디버그(현재 구성)(`debug.current`, `⌃D`)와 🐞 단추는 M6-07 의 실행 액션 묶음이다 — 그것이 `DebugSessions.start` 를 부른다.

import type { Action } from "@pitwall/keymap";
import type { DebugAction } from "@pitwall/rpc-client";

import type { DebugSessions } from "./debug-session";
import type { DebugStrings } from "./strings";

export const DEBUG_ACTION_IDS = [
  "debug.stepOver",
  "debug.stepInto",
  "debug.stepOut",
  "debug.resume",
  "debug.pause",
  "debug.evaluate",
  "debug.toggleBreakpoint",
  "debug.viewBreakpoints",
] as const;

export interface DebugActionDeps {
  sessions: DebugSessions;
  /** 활성 에디터 탭의 파일 · 커서 줄 · 고른 글(없으면 `undefined`). */
  editor(): { path: string; line: number; selection: string } | undefined;
  toggleBreakpoint(path: string, line: number): void;
  viewBreakpoints(): void;
  failed(message: string): void;
}

export function debugActions(T: DebugStrings["actions"], deps: DebugActionDeps): Action[] {
  const { sessions } = deps;
  const control = (action: DebugAction) => () =>
    sessions
      .control(action)
      .catch((e: unknown) => deps.failed(e instanceof Error ? e.message : String(e)));
  const paused = () => sessions.paused();
  return [
    {
      id: "debug.stepOver",
      label: T.stepOver,
      keywords: ["step over", "debug"],
      enabled: paused,
      run: control("next"),
      repeat: true,
    },
    {
      id: "debug.stepInto",
      label: T.stepInto,
      keywords: ["step into", "debug"],
      enabled: paused,
      run: control("stepIn"),
      repeat: true,
    },
    {
      id: "debug.stepOut",
      label: T.stepOut,
      keywords: ["step out", "debug"],
      enabled: paused,
      run: control("stepOut"),
    },
    {
      id: "debug.resume",
      label: T.resume,
      keywords: ["resume", "continue", "debug"],
      enabled: paused,
      run: control("continue"),
    },
    {
      id: "debug.pause",
      label: T.pause,
      keywords: ["pause", "debug"],
      enabled: () => sessions.debugging() && !sessions.paused(),
      run: control("pause"),
    },
    {
      id: "debug.evaluate",
      label: T.evaluate,
      keywords: ["evaluate", "expression", "debug"],
      enabled: () => sessions.debugging(),
      run: () => sessions.showConsole(deps.editor()?.selection ?? ""),
    },
    {
      id: "debug.toggleBreakpoint",
      label: T.toggleBreakpoint,
      keywords: ["breakpoint", "toggle"],
      enabled: () => deps.editor() !== undefined,
      run: () => {
        const at = deps.editor();
        if (at) deps.toggleBreakpoint(at.path, at.line);
      },
    },
    {
      id: "debug.viewBreakpoints",
      label: T.viewBreakpoints,
      keywords: ["breakpoints", "list"],
      run: () => deps.viewBreakpoints(),
    },
  ];
}
