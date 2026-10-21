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

/** 기본 키맵 + 모든 액션이 등록된 창. `terminal` 이면 터미널이 포커스를 가졌다. 시계는 `clock.now` 다. */
const clock = { now: 0 };
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
    now: () => clock.now,
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

describe("Search Everywhere keys while the terminal has focus", () => {
  it("open Search Everywhere and its tabs (macOS)", () => {
    const t = engine("mac", true);
    expect(actionOf(t, key("KeyO", { metaKey: true, shiftKey: true }))).toBe("search.gotoFile");
    expect(actionOf(t, key("KeyE", { metaKey: true }))).toBe("search.recentFiles");
    expect(actionOf(t, key("KeyP", { metaKey: true, shiftKey: true }))).toBe("project.switcher");
  });

  it("take Ctrl+Shift+N · Ctrl+Shift+P but leave Ctrl+N · Ctrl+P · Ctrl+E to the shell (Windows · Linux)", () => {
    for (const p of ["win", "linux"] as const) {
      const t = engine(p, true);
      expect(actionOf(t, key("KeyN", { ctrlKey: true, shiftKey: true })), p).toBe(
        "search.gotoFile",
      );
      expect(actionOf(t, key("KeyP", { ctrlKey: true, shiftKey: true })), p).toBe(
        "project.switcher",
      );
      for (const code of ["KeyN", "KeyP", "KeyE"]) {
        const k = key(code, { ctrlKey: true });
        expect(t.peek(k), `${p} ${code}`).toBe(false);
        expect(actionOf(t, k), `${p} ${code}`).toBeUndefined();
      }
      // 터미널 밖에서는 Ctrl+E 가 최근 파일이다.
      expect(actionOf(engine(p, false), key("KeyE", { ctrlKey: true })), p).toBe(
        "search.recentFiles",
      );
    }
  });

  /** 수정 키를 누르고 뗀다(`ms` 뒤에 뗀다). 뗄 때의 결과를 돌려준다. */
  function tap(t: KeymapEngine, code: string, ms = 50, mods: Partial<KeyEventLike> = {}) {
    t.keydown(key(code, { shiftKey: true, ...mods }));
    clock.now += ms;
    const up = t.keyup(key(code, mods));
    clock.now += ms;
    return up.action;
  }

  it("open on Shift Shift, but not when a key comes between the two Shifts", () => {
    const t = engine("mac", true);
    expect(tap(t, "ShiftLeft")).toBeUndefined();
    expect(tap(t, "ShiftLeft")).toBe("search.everywhere");

    // 대문자 두 글자(A B): Shift 를 누른 채 A, 떼고, 다시 Shift 를 누른 채 B.
    t.keydown(key("ShiftLeft", { shiftKey: true }));
    t.keydown(key("KeyA", { shiftKey: true }));
    clock.now += 30;
    expect(t.keyup(key("ShiftLeft")).action).toBeUndefined();
    t.keydown(key("ShiftLeft", { shiftKey: true }));
    t.keydown(key("KeyB", { shiftKey: true }));
    clock.now += 30;
    expect(t.keyup(key("ShiftLeft")).action).toBeUndefined();
    // Shift 한 번 뒤 글자, 그다음 Shift 한 번도 아니다.
    expect(tap(t, "ShiftLeft")).toBeUndefined();
    t.keydown(key("KeyA"));
    expect(tap(t, "ShiftLeft")).toBeUndefined();
  });

  it("do nothing while the IME is composing", () => {
    const t = engine("mac", true);
    expect(tap(t, "ShiftLeft", 50, { isComposing: true })).toBeUndefined();
    expect(tap(t, "ShiftLeft", 50, { isComposing: true })).toBeUndefined();
    for (const k of [
      key("KeyO", { metaKey: true, shiftKey: true, isComposing: true }),
      key("KeyE", { metaKey: true, isComposing: true }),
      key("KeyP", { metaKey: true, shiftKey: true, isComposing: true }),
    ]) {
      expect(t.peek(k), k.code).toBe(false);
      expect(actionOf(t, k), k.code).toBeUndefined();
    }
  });
});
