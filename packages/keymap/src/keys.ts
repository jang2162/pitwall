// 키 표기(docs/design/10-ui.md §4.5 「키 표기」)를 물리 키 조합으로 읽고, 표시 라벨 · 네이티브 메뉴 accelerator 로 바꾼다.
//
// 매칭은 `KeyboardEvent.code`(물리 키 자리)로 한다(§4.4 「물리 키 기준」). 한글 입력 상태에서 ⌘⇧A 의 `key` 는 "ㅁ" 으로
// 오지만 `code` 는 "KeyA" 다. 그래서 키 이름은 모두 US 배열 자리의 `code` 로 읽는다 — `"a"` 는 「A 자리의 키」다.
// 그 대가로 AZERTY · Dvorak 처럼 글자 자리가 다른 배열에서는 표기의 글자와 누르는 글자가 다르다. 표시 라벨만은
// 배열 지도(`LayoutMap`)가 있으면 그 배열의 글자로 보여 준다.

export type Platform = "mac" | "win" | "linux";

export type Modifier = "cmd" | "ctrl" | "alt" | "shift";

/** 키 하나를 누른 순간: 수정 키 묶음 + 물리 키(`KeyboardEvent.code`). */
export interface Stroke {
  readonly cmd: boolean;
  readonly ctrl: boolean;
  readonly alt: boolean;
  readonly shift: boolean;
  readonly code: string;
}

/**
 * 단축키 하나.
 * - `strokes`: 한 번 누르기(길이 1) 또는 연속 키(chord, `"cmd+k cmd+s"`, 길이 2 이상).
 * - `double`: 수정 키 더블 탭(`"shift shift"`). 수정 키만 된다 — 글자 키 두 번은 타이핑과 구별되지 않는다.
 *   설계의 「`⌥⌥` 누른 채 방향키」(멀티 커서)는 아직 받지 않는다 — 그 기능이 생길 때 형식을 더한다.
 */
export type KeySequence =
  | { readonly kind: "strokes"; readonly strokes: readonly Stroke[] }
  | { readonly kind: "double"; readonly modifier: Modifier };

const MODIFIER_ALIASES: Record<string, Modifier> = {
  cmd: "cmd",
  command: "cmd",
  meta: "cmd",
  super: "cmd",
  win: "cmd",
  ctrl: "ctrl",
  control: "ctrl",
  alt: "alt",
  option: "alt",
  opt: "alt",
  shift: "shift",
};

const NAMED_CODES: Record<string, string> = {
  enter: "Enter",
  return: "Enter",
  escape: "Escape",
  esc: "Escape",
  space: "Space",
  tab: "Tab",
  backspace: "Backspace",
  delete: "Delete",
  insert: "Insert",
  home: "Home",
  end: "End",
  pageup: "PageUp",
  pagedown: "PageDown",
  up: "ArrowUp",
  down: "ArrowDown",
  left: "ArrowLeft",
  right: "ArrowRight",
  ",": "Comma",
  comma: "Comma",
  ".": "Period",
  period: "Period",
  "/": "Slash",
  slash: "Slash",
  ";": "Semicolon",
  semicolon: "Semicolon",
  "'": "Quote",
  quote: "Quote",
  "[": "BracketLeft",
  "]": "BracketRight",
  "\\": "Backslash",
  backslash: "Backslash",
  "-": "Minus",
  minus: "Minus",
  "=": "Equal",
  equal: "Equal",
  "`": "Backquote",
  backquote: "Backquote",
};

/** 수정 키 자체의 `code`. 더블 탭을 알아보고, 수정 키만 눌린 keydown 을 단축키 매칭에서 뺀다. */
export const MODIFIER_CODES: Readonly<Record<string, Modifier>> = {
  MetaLeft: "cmd",
  MetaRight: "cmd",
  OSLeft: "cmd",
  OSRight: "cmd",
  ControlLeft: "ctrl",
  ControlRight: "ctrl",
  AltLeft: "alt",
  AltRight: "alt",
  ShiftLeft: "shift",
  ShiftRight: "shift",
};

export class KeyParseError extends Error {
  constructor(text: string, reason: string) {
    super(`"${text}": ${reason}`);
    this.name = "KeyParseError";
  }
}

function keyCode(name: string): string | undefined {
  if (/^[a-z]$/.test(name)) return `Key${name.toUpperCase()}`;
  if (/^[0-9]$/.test(name)) return `Digit${name}`;
  const f = /^f([1-9]|1[0-9]|2[0-4])$/.exec(name);
  if (f) return `F${f[1]}`;
  return NAMED_CODES[name];
}

function parseStroke(text: string, token: string): Stroke {
  // "cmd++" 같은 표기는 받지 않는다 — `+` 는 물리 키가 아니라 shift+= 의 글자다.
  const parts = token.split("+");
  if (parts.some((p) => p === "")) throw new KeyParseError(text, `빈 자리가 있다: "${token}"`);
  const mods = { cmd: false, ctrl: false, alt: false, shift: false };
  let code: string | undefined;
  for (const raw of parts) {
    const part = raw.toLowerCase();
    const modifier = MODIFIER_ALIASES[part];
    if (modifier) {
      if (mods[modifier]) throw new KeyParseError(text, `수정 키가 두 번 있다: ${part}`);
      mods[modifier] = true;
      continue;
    }
    if (code !== undefined) throw new KeyParseError(text, `키가 둘 이상이다: "${token}"`);
    if (part === "click" || /^mouse\d$/.test(part)) {
      throw new KeyParseError(text, "마우스 단축키는 아직 지원하지 않는다");
    }
    code = keyCode(part);
    if (code === undefined) throw new KeyParseError(text, `모르는 키 이름: ${raw}`);
  }
  if (code === undefined) throw new KeyParseError(text, `수정 키만 있다: "${token}"`);
  return { ...mods, code };
}

/** `"cmd+shift+a"`, `"shift shift"`, `"cmd+k cmd+s"` 를 읽는다. 대소문자는 가리지 않는다. 틀리면 `KeyParseError`. */
export function parseKeySequence(text: string): KeySequence {
  const tokens = text.trim().split(/\s+/).filter(Boolean);
  if (tokens.length === 0) throw new KeyParseError(text, "비어 있다");
  const lone = tokens.map((t) => MODIFIER_ALIASES[t.toLowerCase()]);
  if (lone.some(Boolean)) {
    if (tokens.length === 2 && lone[0] && lone[0] === lone[1]) {
      return { kind: "double", modifier: lone[0] };
    }
    throw new KeyParseError(text, "수정 키만 누르는 것은 같은 수정 키 두 번(더블 탭)만 된다");
  }
  return { kind: "strokes", strokes: tokens.map((t) => parseStroke(text, t)) };
}

export function strokeKey(s: Stroke): string {
  return `${s.cmd ? "C" : ""}${s.ctrl ? "T" : ""}${s.alt ? "A" : ""}${s.shift ? "S" : ""}:${s.code}`;
}

/** 같은 키인지. 표기가 달라도(`"cmd+shift+a"` · `"shift+cmd+A"`) 같은 키면 같다. */
export function sameSequence(a: KeySequence, b: KeySequence): boolean {
  if (a.kind === "double" || b.kind === "double") {
    return a.kind === "double" && b.kind === "double" && a.modifier === b.modifier;
  }
  return (
    a.strokes.length === b.strokes.length &&
    a.strokes.every((s, i) => strokeKey(s) === strokeKey(b.strokes[i]!))
  );
}

/** 키 이벤트에서 읽은 누름. 수정 키만 눌린 keydown 이면 `undefined`. */
export function strokeFromEvent(e: {
  code: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}): Stroke | undefined {
  if (!e.code || e.code in MODIFIER_CODES) return undefined;
  return { cmd: e.metaKey, ctrl: e.ctrlKey, alt: e.altKey, shift: e.shiftKey, code: e.code };
}

/**
 * `code` → 그 키가 지금 배열에서 내는 글자. 브라우저의 `navigator.keyboard.getLayoutMap()` 과 같은 모양이다.
 * ⚠ WebKit(macOS 웹뷰)에는 `navigator.keyboard` 가 없다 — 그때는 지도 없이 US 배열 글자로 보인다.
 */
export type LayoutMap = ReadonlyMap<string, string>;

const MAC_MODIFIER_LABELS: [Modifier, string][] = [
  ["ctrl", "⌃"],
  ["alt", "⌥"],
  ["shift", "⇧"],
  ["cmd", "⌘"],
];

const MAC_KEY_LABELS: Record<string, string> = {
  Enter: "↩",
  Escape: "⎋",
  Tab: "⇥",
  Backspace: "⌫",
  Delete: "⌦",
  Space: "Space",
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  PageUp: "⇞",
  PageDown: "⇟",
  Home: "↖",
  End: "↘",
};

const OTHER_KEY_LABELS: Record<string, string> = {
  Escape: "Esc",
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
};

const US_LABELS: Record<string, string> = {
  Comma: ",",
  Period: ".",
  Slash: "/",
  Semicolon: ";",
  Quote: "'",
  BracketLeft: "[",
  BracketRight: "]",
  Backslash: "\\",
  Minus: "-",
  Equal: "=",
  Backquote: "`",
};

function codeLabel(code: string, platform: Platform, layout?: LayoutMap): string {
  const special = (platform === "mac" ? MAC_KEY_LABELS : OTHER_KEY_LABELS)[code];
  if (special) return special;
  const printed = layout?.get(code);
  if (printed && printed.trim()) return printed.toUpperCase();
  if (code.startsWith("Key")) return code.slice(3);
  if (code.startsWith("Digit")) return code.slice(5);
  return US_LABELS[code] ?? code;
}

function otherModifierName(m: Modifier, platform: Platform): string {
  if (m === "cmd") return platform === "win" ? "Win" : "Super";
  return { ctrl: "Ctrl", alt: "Alt", shift: "Shift" }[m];
}

function strokeLabel(s: Stroke, platform: Platform, layout?: LayoutMap): string {
  const key = codeLabel(s.code, platform, layout);
  if (platform === "mac") {
    return MAC_MODIFIER_LABELS.filter(([m]) => s[m])
      .map(([, l]) => l)
      .join("")
      .concat(key);
  }
  const mods = (["ctrl", "alt", "shift", "cmd"] as const).filter((m) => s[m]);
  return [...mods.map((m) => otherModifierName(m, platform)), key].join("+");
}

/** 사람에게 보이는 라벨. macOS 는 `⌘⇧A`, 그 밖은 `Ctrl+Shift+A`. 연속 키는 `, ` 로 잇는다(IntelliJ 표기). */
export function formatSequence(seq: KeySequence, platform: Platform, layout?: LayoutMap): string {
  if (seq.kind === "double") {
    if (platform === "mac")
      return MAC_MODIFIER_LABELS.find(([m]) => m === seq.modifier)![1].repeat(2);
    const name = otherModifierName(seq.modifier, platform);
    return `${name} ${name}`;
  }
  return seq.strokes.map((s) => strokeLabel(s, platform, layout)).join(", ");
}

// 그 OS 의 키보드에 없는 키. Mac 키보드에는 Insert 가 없다(메뉴에 넣으면 빈 글자 + ⌘ 로 보였다).
const MISSING_CODES: Record<Platform, ReadonlySet<string>> = {
  mac: new Set(["Insert"]),
  win: new Set(),
  linux: new Set(),
};

/** 그 OS 에서 누를 수 없는 키가 들었으면 그 키의 `code`. 아니면 `undefined`. */
export function missingKey(seq: KeySequence, platform: Platform): string | undefined {
  if (seq.kind !== "strokes") return undefined;
  return seq.strokes.find((s) => MISSING_CODES[platform].has(s.code))?.code;
}

/**
 * Tauri(muda) 메뉴 accelerator 문자열. 한 번 누르기만 된다 — 연속 키 · 더블 탭은 메뉴가 표현하지 못해 `undefined`.
 * `platform` 을 주면 그 OS 에 없는 키(`missingKey`)도 `undefined` 다.
 * 키 이름은 `code` 를 그대로 쓴다(muda 가 `KeyA` · `Digit1` · `Comma` 를 받는다). `cmd` 는 `Super` 다 — macOS 에서 ⌘,
 * 그 밖에서 Win 키다. `CmdOrCtrl` 는 쓰지 않는다 — OS별 차이는 키맵 파일이 이미 갈라 두었다.
 */
export function toAccelerator(seq: KeySequence, platform?: Platform): string | undefined {
  if (seq.kind !== "strokes" || seq.strokes.length !== 1) return undefined;
  if (platform && missingKey(seq, platform)) return undefined;
  const s = seq.strokes[0]!;
  const mods = [s.cmd && "Super", s.ctrl && "Control", s.alt && "Alt", s.shift && "Shift"].filter(
    (m): m is string => Boolean(m),
  );
  return [...mods, s.code].join("+");
}

// 배정해도 앱에 오지 않는 OS 키(§4.4 「OS 예약 키」). 다 모은 목록이 아니라 흔히 부딪히는 것만이다.
const RESERVED: Record<Platform, [string, string][]> = {
  mac: [
    ["cmd+h", "앱 가리기"],
    ["cmd+m", "창 최소화"],
    ["cmd+q", "앱 종료"],
    ["cmd+tab", "앱 전환"],
    ["cmd+space", "Spotlight"],
    ["ctrl+space", "입력 소스 전환"],
    ["ctrl+up", "Mission Control"],
    ["ctrl+down", "앱 윈도우"],
    ["cmd+alt+escape", "강제 종료"],
  ],
  win: [
    ["alt+tab", "앱 전환"],
    ["alt+f4", "창 닫기"],
    ["ctrl+alt+delete", "보안 화면"],
    ["cmd+l", "잠금"],
    ["cmd+d", "바탕 화면 보기"],
  ],
  linux: [
    ["alt+tab", "앱 전환"],
    ["alt+f4", "창 닫기"],
  ],
};

/** OS 가 가져가 앱에 오지 않는 키면 그 이유. 아니면 `undefined`. */
export function reservedReason(seq: KeySequence, platform: Platform): string | undefined {
  if (seq.kind !== "strokes" || seq.strokes.length !== 1) return undefined;
  for (const [text, reason] of RESERVED[platform]) {
    if (sameSequence(seq, parseKeySequence(text))) return reason;
  }
  return undefined;
}
