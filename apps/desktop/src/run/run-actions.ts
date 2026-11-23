// 실행 키맵 액션(17-m6-plan M6-07 ② — 10-ui.md 「키맵」 의 「실행 · 디버그」 표, 기본 키는
// extensions/keymap-default/keymaps/default.toml). 모두 액션 찾기(⌘⇧A)에 든다. 툴 윈도우 액션 `toolWindow.run`(⌘4) ·
// `toolWindow.debug`(⌘5)는 툴 윈도우 표(keymap/actions.ts 의 `toolWindowActions`)가 이미 등록한다.
//
// - `run.current`(⌃R): 지금 구성을 띄운다(첫 실행 확인 · `sql` 의 위험 확인을 지난다).
// - `debug.current`(⌃D): 지금 구성의 `[debug]` 로 디버그 세션을 연다(M6-18 — `DebugSessions.start`). `[debug]` 가 없는 구성이면
//   꺼진다 — 키도 가져가지 않는다.
// - `run.choose`(⌃⌥R): 구성을 골라 바로 띄우는 팝업.
// - `run.stop`(⌘F2): **`dbConsoleFocus` 밖에서만**(키맵 파일의 `when`) — DB 콘솔 안의 ⌘F2 는 쿼리 취소다(10-ui.md 「키맵」).
// - `run.editConfigs`: 구성 편집을 연다(기본 키 없음).

import type { Action } from "@pitwall/keymap";

import type { RunStrings } from "./strings";

export const RUN_ACTION_IDS = [
  "run.current",
  "debug.current",
  "run.choose",
  "run.chooseIn",
  "run.stop",
  "run.editConfigs",
] as const;

export function runActions(deps: {
  strings: RunStrings;
  /** 지금 구성이 있고 쓸 수 있다. */
  canRun(): boolean;
  runCurrent(): void;
  /** 지금 구성이 쓸 수 있고 `[debug]` 가 있다. */
  canDebug(): boolean;
  debugCurrent(): void;
  chooseAndRun(): void;
  /** 그 체크아웃(`{ checkout: CheckoutRef }`)에서 골라 실행 — M7-17 이 부른다. */
  chooseIn(args: unknown): void;
  /** 멈출 실행이 있다. */
  canStop(): boolean;
  stop(): void;
  editConfigs(): void;
}): Action[] {
  const T = deps.strings.actions;
  return [
    {
      id: "run.current",
      label: T.current,
      keywords: ["Run"],
      enabled: deps.canRun,
      run: deps.runCurrent,
    },
    {
      id: "debug.current",
      label: T.debug,
      keywords: ["Debug"],
      enabled: deps.canDebug,
      run: deps.debugCurrent,
    },
    {
      id: "run.choose",
      label: T.choose,
      keywords: ["Run...", "Run Configuration"],
      run: deps.chooseAndRun,
    },
    {
      // 인자가 있어야 돈다(`args`) — 키 · 메뉴 · 액션 찾기에서는 꺼진다. 인자 없는 고르기는 `run.choose`.
      id: "run.chooseIn",
      label: T.chooseIn,
      keywords: ["Run in Checkout"],
      args: true,
      run: (args?: unknown) => deps.chooseIn(args),
    },
    {
      id: "run.stop",
      label: T.stop,
      keywords: ["Stop"],
      enabled: deps.canStop,
      run: deps.stop,
    },
    {
      id: "run.editConfigs",
      label: T.edit,
      keywords: ["Edit Configurations"],
      run: deps.editConfigs,
    },
  ];
}
