import { describe, expect, it } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { parseTheme } from "../theme/tokens";
import { ANSI_COLORS, terminalTheme } from "./terminal-theme";

describe("terminal colours", () => {
  const theme = parseTheme(defaultTheme);

  it("come from the theme tokens, all 16 ANSI colours included", () => {
    const light = terminalTheme(theme.variants.light!);
    expect(light.background).toBe(theme.variants.light!["terminal.background"]);
    expect(light.foreground).toBe(theme.variants.light!["terminal.foreground"]);
    expect(light.cursor).toBe(theme.variants.light!["terminal.cursor"]);
    expect(light.selectionBackground).toBe(theme.variants.light!["terminal.selectionBackground"]);
    for (const color of ANSI_COLORS) {
      expect(light[color], color).toBe(theme.variants.light![`terminal.ansi.${color}`]);
    }
  });

  it("differ between the light and dark variants of the default theme", () => {
    const light = terminalTheme(theme.variants.light!);
    const dark = terminalTheme(theme.variants.dark!);
    expect(light.background).not.toBe(dark.background);
    expect(light.foreground).not.toBe(dark.foreground);
    // `ls --color` 의 디렉터리(파랑)도 외관을 따른다.
    expect(light.blue).not.toBe(dark.blue);
  });
});
