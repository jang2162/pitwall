// 디자인 토큰(docs/design/07-extensibility.md §3 「테마」)과 그 값을 CSS 변수로 주입하는 규칙.
//
// 토큰 이름(`ui.background`)은 테마 파일의 키이고, 화면은 CSS 변수(`--pw-ui-background`)로만 읽는다. 기본 테마는 내장
// 확장의 파일(extensions/theme-default/themes/default.json)이다.

import type { SyntaxToken } from "@pitwall/rpc-client";

/**
 * 구문 토큰(구문 분석 서비스가 캡처 이름을 바꿔 내보내는 이름, crates/pw-syntax token.rs). 테마 토큰은 `syntax.<토큰>` 이다.
 * 서비스에 토큰이 생기면 아래 검사가 타입 오류를 낸다 — 여기와 기본 테마에 함께 더한다.
 */
export const SYNTAX_TOKENS = [
  "attribute",
  "boolean",
  "comment",
  "comment.documentation",
  "constant",
  "constant.builtin",
  "constructor",
  "embedded",
  "function",
  "function.builtin",
  "function.macro",
  "function.method",
  "keyword",
  "label",
  "markup.bold",
  "markup.heading",
  "markup.italic",
  "markup.link",
  "markup.link.url",
  "markup.raw",
  "module",
  "number",
  "operator",
  "property",
  "punctuation",
  "punctuation.bracket",
  "punctuation.delimiter",
  "punctuation.special",
  "string",
  "string.escape",
  "string.regexp",
  "string.special",
  "string.special.key",
  "tag",
  "type",
  "type.builtin",
  "variable",
  "variable.builtin",
  "variable.parameter",
] as const satisfies readonly SyntaxToken[];

type MissingSyntaxToken = Exclude<SyntaxToken, (typeof SYNTAX_TOKENS)[number]>;
// 빠진 토큰이 있으면 `true` 를 `never` 에 넣지 못해 타입 오류다.
const syntaxTokensComplete: [MissingSyntaxToken] extends [never] ? true : never = true;
void syntaxTokensComplete;

/** 구문 토큰의 테마 토큰 이름. */
export const syntaxTokenName = (token: SyntaxToken): `syntax.${SyntaxToken}` => `syntax.${token}`;

/** 화면이 읽는 토큰. 테마 파일의 변형(light·dark)은 있으면 이것을 전부 정의해야 한다. */
export const TOKENS = [
  "ui.background",
  "ui.foreground",
  "ui.mutedForeground",
  "ui.border",
  "ui.panelBackground",
  "ui.toolbarBackground",
  "ui.statusBarBackground",
  "ui.statusBarForeground",
  "ui.hoverBackground",
  "ui.activeBackground",
  "ui.accent",
  "ui.accentForeground",
  "tab.activeBackground",
  "tab.activeForeground",
  "tab.inactiveBackground",
  "tab.inactiveForeground",
  "editor.background",
  "editor.foreground",
  // 터미널(xterm.js) 색. xterm 은 CSS 변수가 아니라 테마 객체로 칠한다(src/terminal/terminal-theme.ts).
  "terminal.background",
  "terminal.foreground",
  "terminal.cursor",
  "terminal.selectionBackground",
  "terminal.ansi.black",
  "terminal.ansi.red",
  "terminal.ansi.green",
  "terminal.ansi.yellow",
  "terminal.ansi.blue",
  "terminal.ansi.magenta",
  "terminal.ansi.cyan",
  "terminal.ansi.white",
  "terminal.ansi.brightBlack",
  "terminal.ansi.brightRed",
  "terminal.ansi.brightGreen",
  "terminal.ansi.brightYellow",
  "terminal.ansi.brightBlue",
  "terminal.ansi.brightMagenta",
  "terminal.ansi.brightCyan",
  "terminal.ansi.brightWhite",
  // 구문 색(M1-17). 에디터의 강조가 CSS 변수로 읽는다(src/editor/extensions/highlight.ts).
  ...SYNTAX_TOKENS.map(syntaxTokenName),
] as const;

export type TokenName = (typeof TOKENS)[number];
export type Appearance = "light" | "dark";
export type TokenValues = Record<TokenName, string>;

export interface Theme {
  id: string;
  label: string;
  /** 하나 이상 있다. 어느 외관을 칠할지는 `resolveAppearance`. */
  variants: Partial<Record<Appearance, TokenValues>>;
}

const APPEARANCES = ["light", "dark"] as const;

// 마디는 영숫자만 받는다. `-` 를 허용하면 `a.b-c` 와 `a.b.c` 가 같은 변수(`--pw-a-b-c`)가 된다.
// 대소문자는 그대로 둔다 — CSS 사용자 정의 속성은 대소문자를 가리므로 `ui.fooBar` 와 `ui.foo.bar` 가 갈린다.
const TOKEN_NAME = /^[A-Za-z][A-Za-z0-9]*(\.[A-Za-z0-9]+)+$/;

/** 토큰 이름 → CSS 변수 이름. `ui.mutedForeground` → `--pw-ui-mutedForeground`. */
export function tokenCssVar(name: string): string {
  if (!TOKEN_NAME.test(name)) throw new Error(`토큰 이름이 아니다: ${name}`);
  return `--pw-${name.replaceAll(".", "-")}`;
}

/**
 * 테마 파일(JSON)을 검사해 읽는다. light · dark 변형 중 하나 이상이 있어야 하고, 있는 변형은 TOKENS 를 전부 문자열로
 * 정의해야 한다. 모르는 토큰은 버린다 — 더 새 앱을 위해 만든 테마가 이 앱에서도 열려야 하고, 주입되는 변수 집합이
 * 테마마다 달라지면 앞 테마의 값이 남는다.
 */
export function parseTheme(input: unknown): Theme {
  if (!isObject(input)) throw new Error("테마 파일이 객체가 아니다");
  const { id, label, variants } = input;
  if (typeof id !== "string" || id === "") throw new Error("테마 id 가 없다");
  if (typeof label !== "string") throw new Error(`테마 ${id}: label 이 문자열이 아니다`);
  if (!isObject(variants)) throw new Error(`테마 ${id}: variants 가 객체가 아니다`);
  const parsed: Theme["variants"] = {};
  for (const appearance of APPEARANCES) {
    const values = variants[appearance];
    if (values === undefined) continue;
    if (!isObject(values)) throw new Error(`테마 ${id}: ${appearance} 변형이 객체가 아니다`);
    const tokens = {} as TokenValues;
    for (const token of TOKENS) {
      const value = values[token];
      if (typeof value !== "string" || value.trim() === "") {
        throw new Error(`테마 ${id}: ${appearance}.${token} 이 없다`);
      }
      tokens[token] = value;
    }
    parsed[appearance] = tokens;
  }
  if (!parsed.light && !parsed.dark) throw new Error(`테마 ${id}: light · dark 변형이 하나도 없다`);
  return { id, label, variants: parsed };
}

/** 테마에 있는 변형. */
export function themeAppearances(theme: Theme): Appearance[] {
  return APPEARANCES.filter((appearance) => theme.variants[appearance]);
}

/**
 * 고른 외관(`wanted`)에 실제로 칠할 변형. 한쪽 변형만 있는 테마는 어느 외관을 골라도 그 변형을 칠한다 — 규칙의 정본은
 * docs/design/07-extensibility.md 「3. 테마」 「형식」 행.
 */
export function resolveAppearance(theme: Theme, wanted: Appearance): Appearance {
  if (theme.variants[wanted]) return wanted;
  return wanted === "light" ? "dark" : "light";
}

/** `style` 은 보통 `document.documentElement.style` 이다. */
export function applyTokens(style: Pick<CSSStyleDeclaration, "setProperty">, values: TokenValues) {
  for (const token of TOKENS) style.setProperty(tokenCssVar(token), values[token]);
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
