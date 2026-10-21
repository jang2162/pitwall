import {
  type ContextLayers,
  KeymapEngine,
  type KeyEventLike,
  parseKeymapToml,
  type Platform,
  readDefaultKeymap,
} from "@pitwall/keymap";
import { describe, expect, it } from "vitest";

import { DEFAULT_KEYMAP_TOML } from "../keymap/default-keymap";
import { TERMINAL_FOCUS, terminalAdmits } from "./terminal-keys";

const NO_MODS = { metaKey: false, ctrlKey: false, altKey: false, shiftKey: false };
const key = (code: string, mods: Partial<KeyEventLike> = {}): KeyEventLike => ({
  code,
  key: code,
  ...NO_MODS,
  ...mods,
});

/** 기본 키맵 + 모든 액션이 등록된 창. `terminal` 이면 터미널이 포커스를 가졌다. */
function engine(platform: Platform, terminal: boolean) {
  const layers: ContextLayers = [
    terminal ? { [TERMINAL_FOCUS]: true, inputFocus: true } : {},
    terminal ? { toolWindowFocus: true, activeToolWindow: "terminal" } : {},
    { isMac: platform === "mac" },
  ];
  const e = new KeymapEngine({
    run: () => true,
    isEnabled: () => true,
    context: () => layers,
    admits: terminalAdmits,
  });
  e.setBindings(readDefaultKeymap(parseKeymapToml(DEFAULT_KEYMAP_TOML), platform));
  return e;
}

const actionOf = (e: KeymapEngine, k: KeyEventLike) => e.keydown(k).action;

describe("keys while the terminal has focus", () => {
  it("go to the shell, except terminal actions and window-wide actions (macOS)", () => {
    const t = engine("mac", true);
    // 셸로: Ctrl 조합 · ⌥ · 에디터 액션.
    for (const k of [
      key("KeyC", { ctrlKey: true }),
      key("KeyW", { ctrlKey: true }),
      key("KeyB", { altKey: true }),
      key("Backslash", { metaKey: true }), // editor.splitRight
      key("KeyW", { metaKey: true, altKey: true }), // editor.closeOtherTabs
    ]) {
      expect(t.peek(k), k.code).toBe(false);
      expect(actionOf(t, k), k.code).toBeUndefined();
    }
    // 앱으로: 터미널 액션이 같은 키의 창 액션을 이긴다(⌘W 는 탭을 닫지 창을 닫지 않는다).
    expect(t.peek(key("KeyW", { metaKey: true }))).toBe(true);
    expect(actionOf(t, key("KeyW", { metaKey: true }))).toBe("terminal.closeTab");
    expect(actionOf(t, key("KeyT", { metaKey: true }))).toBe("terminal.newTab");
    expect(actionOf(t, key("F12", { altKey: true }))).toBe("toolWindow.terminal");
    expect(actionOf(t, key("Digit1", { metaKey: true }))).toBe("toolWindow.project");
    expect(actionOf(t, key("KeyA", { metaKey: true, shiftKey: true }))).toBe("action.find");
    expect(actionOf(t, key("Comma", { metaKey: true }))).toBe("settings.open");
  });

  it("leave Ctrl+C to the shell and copy with Ctrl+Shift+C (Windows · Linux)", () => {
    for (const p of ["win", "linux"] as const) {
      const t = engine(p, true);
      expect(actionOf(t, key("KeyC", { ctrlKey: true })), p).toBeUndefined();
      expect(actionOf(t, key("F4", { ctrlKey: true })), p).toBeUndefined(); // editor.closeTab
      expect(actionOf(t, key("KeyC", { ctrlKey: true, shiftKey: true })), p).toBe("terminal.copy");
      expect(actionOf(t, key("KeyV", { ctrlKey: true, shiftKey: true })), p).toBe("terminal.paste");
      expect(actionOf(t, key("Digit1", { altKey: true })), p).toBe("toolWindow.project");
    }
  });

  it("do not change outside the terminal", () => {
    const t = engine("mac", false);
    expect(actionOf(t, key("KeyW", { metaKey: true }))).toBe("window.close");
    expect(actionOf(t, key("Backslash", { metaKey: true }))).toBe("editor.splitRight");
    expect(actionOf(t, key("KeyT", { metaKey: true }))).toBeUndefined();
  });
});
