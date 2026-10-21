// 테마 토큰 → xterm.js 테마(docs/design/07-extensibility.md 「3. 테마」 「일관성」 행). xterm 의 렌더러(WebGL · DOM)는 CSS 변수를
// 읽지 않고 이 객체의 색으로 칠한다 — 그래서 외관이 바뀌면 열린 터미널마다 다시 넘긴다(terminal-tool-window.tsx). 토큰은 다른
// 토큰처럼 CSS 변수(`--pw-terminal-background` 등)로도 들어가 터미널 둘레의 여백이 그것을 쓴다.

import type { ITheme } from "@xterm/xterm";

import type { TokenValues } from "../theme/tokens";

/** ANSI 16색의 이름. 토큰은 `terminal.ansi.<이름>`, xterm 테마의 키는 이름 그대로다. */
export const ANSI_COLORS = [
  "black",
  "red",
  "green",
  "yellow",
  "blue",
  "magenta",
  "cyan",
  "white",
  "brightBlack",
  "brightRed",
  "brightGreen",
  "brightYellow",
  "brightBlue",
  "brightMagenta",
  "brightCyan",
  "brightWhite",
] as const;

export function terminalTheme(values: TokenValues): ITheme {
  const theme: ITheme = {
    background: values["terminal.background"],
    foreground: values["terminal.foreground"],
    cursor: values["terminal.cursor"],
    // 블록 커서 안의 글자는 배경색으로 — 커서와 글자가 같은 색이 되지 않게.
    cursorAccent: values["terminal.background"],
    selectionBackground: values["terminal.selectionBackground"],
  };
  for (const color of ANSI_COLORS) theme[color] = values[`terminal.ansi.${color}`];
  return theme;
}
