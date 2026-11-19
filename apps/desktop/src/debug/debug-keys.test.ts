// @vitest-environment happy-dom
// 디버그 키(17-m6-plan M6-18 ③ · 완료 기준 ④): 스텝 · 계속 · 식 평가 키는 `debugging` 문맥(창이 내는 상태 키 — keymap/context.ts
// `provideContext`)에서만 디버그 액션이다. 기본 키맵 + 창의 문맥 층(포커스에서 만든 것)을 엔진에 그대로 건다.

import {
  KeymapEngine,
  type KeyEventLike,
  type Platform,
  parseKeymapToml,
  readDefaultKeymap,
} from "@pitwall/keymap";
import { afterEach, describe, expect, it } from "vitest";

import { contextLayers, provideContext } from "../keymap/context";
import { DEFAULT_KEYMAP_TOML } from "../keymap/default-keymap";

const NO_MODS = { metaKey: false, ctrlKey: false, altKey: false, shiftKey: false };
const key = (code: string, mods: Partial<KeyEventLike> = {}): KeyEventLike => ({
  code,
  key: code,
  ...NO_MODS,
  ...mods,
});

function engine(platform: Platform) {
  const e = new KeymapEngine({
    run: () => true,
    isEnabled: () => true,
    context: () => contextLayers(document.activeElement, { popupOpen: false, platform }),
  });
  e.setBindings(readDefaultKeymap(parseKeymapToml(DEFAULT_KEYMAP_TOML), platform));
  return e;
}

let off: (() => void) | undefined;
afterEach(() => {
  off?.();
  off = undefined;
  document.body.innerHTML = "";
});

describe("debug keys", () => {
  it("are not debug actions outside the debugging context", () => {
    for (const p of ["mac", "win", "linux"] as const) {
      const e = engine(p);
      expect(e.keydown(key("F8")).action, p).toBeUndefined();
      expect(e.keydown(key("F7")).action, p).toBeUndefined();
      expect(e.keydown(key("F8", { shiftKey: true })).action, p).toBeUndefined();
      expect(e.keydown(key("F8", { altKey: true })).action, p).toBeUndefined();
    }
    expect(
      engine("mac").keydown(key("KeyR", { metaKey: true, altKey: true })).action,
    ).toBeUndefined();
    expect(engine("win").keydown(key("F9")).action).toBeUndefined();
  });

  it("step, resume and evaluate while a session is debugging", () => {
    off = provideContext("debugging", () => true);
    for (const p of ["mac", "win", "linux"] as const) {
      const e = engine(p);
      expect(e.keydown(key("F8")).action, p).toBe("debug.stepOver");
      expect(e.keydown(key("F7")).action, p).toBe("debug.stepInto");
      expect(e.keydown(key("F8", { shiftKey: true })).action, p).toBe("debug.stepOut");
      expect(e.keydown(key("F8", { altKey: true })).action, p).toBe("debug.evaluate");
    }
    expect(engine("mac").keydown(key("KeyR", { metaKey: true, altKey: true })).action).toBe(
      "debug.resume",
    );
    expect(engine("linux").keydown(key("F9")).action).toBe("debug.resume");
  });

  it("leave F7 to the diff view while it has focus", () => {
    off = provideContext("debugging", () => true);
    const diff = document.createElement("div");
    diff.dataset.keymapContext = "diffFocus";
    diff.tabIndex = 0;
    document.body.append(diff);
    diff.focus();
    expect(engine("mac").keydown(key("F7")).action).toBe("diff.nextChange");
  });

  it("toggle a breakpoint in the editor and list them anywhere", () => {
    const editor = document.createElement("div");
    editor.dataset.keymapContext = "editorFocus";
    editor.tabIndex = 0;
    document.body.append(editor);
    editor.focus();
    expect(engine("mac").keydown(key("F8", { metaKey: true })).action).toBe(
      "debug.toggleBreakpoint",
    );
    expect(engine("win").keydown(key("F8", { ctrlKey: true })).action).toBe(
      "debug.toggleBreakpoint",
    );
    editor.blur();
    expect(engine("mac").keydown(key("F8", { metaKey: true, shiftKey: true })).action).toBe(
      "debug.viewBreakpoints",
    );
  });
});
