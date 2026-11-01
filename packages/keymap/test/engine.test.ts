import { describe, expect, it } from "vitest";

import { KeymapEngine, type KeyEventLike } from "../src/engine";
import {
  type Binding,
  mergeKeymap,
  parseKeymapToml,
  readDefaultKeymap,
  readUserKeymap,
} from "../src/keymap-file";
import type { ContextLayers } from "../src/when";

function keymap(toml: string): Binding[] {
  return readDefaultKeymap(parseKeymapToml(toml), "mac");
}

function setup(
  bindings: Binding[],
  { disabled = [] as string[], repeatable = [] as string[] } = {},
) {
  let clock = 0;
  let layers: ContextLayers = [{}, {}, {}];
  const ran: string[] = [];
  const engine = new KeymapEngine({
    run: (action) => {
      if (disabled.includes(action)) return false;
      ran.push(action);
      return true;
    },
    isEnabled: (action) => !disabled.includes(action),
    allowsRepeat: (action) => repeatable.includes(action),
    context: () => layers,
    now: () => clock,
    chordTimeoutMs: 1000,
    doubleTapMs: 300,
  });
  engine.setBindings(bindings);
  return {
    engine,
    ran,
    tick(ms: number) {
      clock += ms;
    },
    setContext(next: ContextLayers) {
      layers = next;
    },
  };
}

const NO_MODS = { metaKey: false, ctrlKey: false, altKey: false, shiftKey: false };

function key(
  code: string,
  mods: Partial<KeyEventLike> = {},
  rest: Partial<KeyEventLike> = {},
): KeyEventLike {
  return { code, key: code, ...NO_MODS, ...mods, ...rest };
}

const cmdShift = { metaKey: true, shiftKey: true };

describe("KeymapEngine — single keys", () => {
  const bindings = keymap(`
[[bindings]]
action = "action.find"
key = "cmd+shift+a"
[[bindings]]
action = "toolWindow.project"
key = "cmd+1"
`);

  it("matches by code when the IME turns the key into hangul", () => {
    const { engine, ran } = setup(bindings);
    expect(engine.keydown(key("KeyA", cmdShift, { key: "ㅁ" }))).toEqual({
      handled: true,
      action: "action.find",
    });
    expect(ran).toEqual(["action.find"]);
  });

  it("does not match by key text", () => {
    const { engine, ran } = setup(bindings);
    // 다른 자리의 키가 "a" 를 내도(배열이 다를 때) A 자리가 아니면 아니다.
    expect(engine.keydown(key("KeyQ", cmdShift, { key: "a" })).handled).toBe(false);
    expect(ran).toEqual([]);
  });

  it("needs exactly the bound modifiers", () => {
    const { engine } = setup(bindings);
    expect(engine.keydown(key("KeyA", { metaKey: true })).handled).toBe(false);
    expect(engine.keydown(key("KeyA", { ...cmdShift, altKey: true })).handled).toBe(false);
  });

  it("ignores keys while the IME is composing, even with ⌘", () => {
    const { engine, ran } = setup(bindings);
    expect(engine.keydown(key("KeyA", cmdShift, { key: "ㅁ", isComposing: true })).handled).toBe(
      false,
    );
    expect(ran).toEqual([]);
  });

  it("ignores a keyCode 229 key without ⌘ or Ctrl (Safari's composition-ending Enter)", () => {
    const { engine, ran } = setup(keymap(`[[bindings]]\naction = "run"\nkey = "shift+enter"\n`));
    expect(engine.keydown(key("Enter", { shiftKey: true }, { keyCode: 229 })).handled).toBe(false);
    expect(ran).toEqual([]);
  });

  it("matches a ⌘ key the IME reports as keyCode 229 when nothing is being composed", () => {
    const { engine, ran } = setup(bindings);
    expect(
      engine.keydown(key("Digit1", { metaKey: true }, { key: "1", keyCode: 229 })).action,
    ).toBe("toolWindow.project");
    expect(ran).toEqual(["toolWindow.project"]);
  });

  it("takes but does not rerun a held key unless the action allows repeat", () => {
    const { engine, ran } = setup(bindings, { repeatable: ["action.find"] });
    engine.keydown(key("Digit1", { metaKey: true }));
    expect(engine.keydown(key("Digit1", { metaKey: true }, { repeat: true }))).toEqual({
      handled: true,
    });
    expect(ran).toEqual(["toolWindow.project"]);
    engine.keydown(key("KeyA", cmdShift));
    engine.keydown(key("KeyA", cmdShift, { repeat: true }));
    expect(ran).toEqual(["toolWindow.project", "action.find", "action.find"]);
  });

  it("leaves keys of actions this window does not run", () => {
    const { engine, ran } = setup(bindings, { disabled: ["toolWindow.project"] });
    expect(engine.keydown(key("Digit1", { metaKey: true })).handled).toBe(false);
    expect(ran).toEqual([]);
  });
});

describe("KeymapEngine — chords", () => {
  const bindings = keymap(`
[[bindings]]
action = "chord.save"
key = "cmd+k cmd+s"
[[bindings]]
action = "single.k"
key = "cmd+k"
[[bindings]]
action = "single.s"
key = "cmd+s"
`);
  const cmd = { metaKey: true };

  it("waits for the second stroke and runs the chord", () => {
    const { engine, ran, tick } = setup(bindings);
    expect(engine.keydown(key("KeyK", cmd))).toEqual({ handled: true, pending: true });
    tick(900);
    expect(engine.keydown(key("KeyS", cmd))).toEqual({ handled: true, action: "chord.save" });
    expect(ran).toEqual(["chord.save"]);
  });

  it("forgets the first stroke after the timeout and reads the next one fresh", () => {
    const { engine, ran, tick } = setup(bindings);
    engine.keydown(key("KeyK", cmd));
    tick(1001);
    expect(engine.keydown(key("KeyS", cmd))).toEqual({ handled: true, action: "single.s" });
    expect(ran).toEqual(["single.s"]);
  });

  it("reads a stroke that does not continue the chord fresh", () => {
    const { engine, ran } = setup(bindings);
    engine.keydown(key("KeyK", cmd));
    expect(engine.keydown(key("KeyS", {}))).toEqual({ handled: false });
    expect(engine.keydown(key("KeyS", cmd)).action).toBe("single.s");
    expect(ran).toEqual(["single.s"]);
  });
});

describe("KeymapEngine — double tap", () => {
  const bindings = keymap(`
[[bindings]]
action = "search.everywhere"
key = "shift shift"
`);
  const shiftDown = (rest: Partial<KeyEventLike> = {}) =>
    key("ShiftLeft", { shiftKey: true }, rest);
  const shiftUp = () => key("ShiftLeft");

  it("runs on the second release within the interval", () => {
    const { engine, ran, tick } = setup(bindings);
    engine.keydown(shiftDown());
    tick(50);
    expect(engine.keyup(shiftUp()).handled).toBe(false);
    tick(250);
    engine.keydown(key("ShiftRight", { shiftKey: true }));
    tick(50);
    expect(engine.keyup(key("ShiftRight"))).toEqual({ handled: true, action: "search.everywhere" });
    expect(ran).toEqual(["search.everywhere"]);
  });

  it("does not run when the second press comes too late", () => {
    const { engine, ran, tick } = setup(bindings);
    engine.keydown(shiftDown());
    tick(50);
    engine.keyup(shiftUp());
    tick(301);
    engine.keydown(shiftDown());
    tick(50);
    expect(engine.keyup(shiftUp()).handled).toBe(false);
    expect(ran).toEqual([]);
  });

  it("does not run when a press is held too long", () => {
    const { engine, ran, tick } = setup(bindings);
    engine.keydown(shiftDown());
    tick(301);
    engine.keyup(shiftUp());
    tick(50);
    engine.keydown(shiftDown());
    tick(50);
    expect(engine.keyup(shiftUp()).handled).toBe(false);
    expect(ran).toEqual([]);
  });

  it("is not a tap when another key goes in between (typing a capital)", () => {
    const { engine, ran, tick } = setup(bindings);
    engine.keydown(shiftDown());
    tick(30);
    engine.keyup(shiftUp());
    tick(30);
    engine.keydown(shiftDown());
    engine.keydown(key("KeyA", { shiftKey: true }));
    tick(30);
    expect(engine.keyup(shiftUp()).handled).toBe(false);
    expect(ran).toEqual([]);
  });

  it("is not a tap with another modifier held", () => {
    const { engine, ran, tick } = setup(bindings);
    engine.keydown(shiftDown({ metaKey: true }));
    tick(30);
    engine.keyup(shiftUp());
    tick(30);
    engine.keydown(shiftDown({ metaKey: true }));
    tick(30);
    expect(engine.keyup(shiftUp()).handled).toBe(false);
    expect(ran).toEqual([]);
  });
});

// 더블 탭 후 누른 채(멀티 커서의 ⌥⌥ 누른 채 ↑ ↓, 10-ui §4.2 「멀티 커서」 · 15-m4-plan M4-14). 한도는 setup 의 doubleTapMs 300.
describe("KeymapEngine — double tap and hold", () => {
  const bindings = keymap(`
[[bindings]]
action = "editor.addCursorBelow"
key = "alt alt+down"
[[bindings]]
action = "editor.addCursorAbove"
key = "alt alt+up"
[[bindings]]
action = "editor.shrinkSelection"
key = "alt+down"
[[bindings]]
action = "alt.double"
key = "alt alt"
`);
  const altDown = (rest: Partial<KeyEventLike> = {}) => key("AltLeft", { altKey: true }, rest);
  const altUp = () => key("AltLeft");
  const arrow = (code: string, rest: Partial<KeyEventLike> = {}) =>
    key(code, { altKey: true }, rest);

  /** 첫 탭(누름 `press` ms) → 간격 `gap` ms → 두 번째 누름(누른 채). */
  function holdAlt(t: ReturnType<typeof setup>, press = 50, gap = 100) {
    t.engine.keydown(altDown());
    t.tick(press);
    t.engine.keyup(altUp());
    t.tick(gap);
    t.engine.keydown(altDown());
  }

  it("runs the held key while the second press is held, as many times as pressed", () => {
    const t = setup(bindings);
    holdAlt(t);
    for (let i = 0; i < 3; i++) {
      t.tick(500); // 누르고 있는 길이에는 한도가 없다
      expect(t.engine.keydown(arrow("ArrowDown"))).toEqual({
        handled: true,
        action: "editor.addCursorBelow",
      });
    }
    expect(t.engine.keydown(arrow("ArrowUp")).action).toBe("editor.addCursorAbove");
    expect(t.ran).toEqual([
      "editor.addCursorBelow",
      "editor.addCursorBelow",
      "editor.addCursorBelow",
      "editor.addCursorAbove",
    ]);
  });

  it("ends when the modifier goes up: the next arrow is the plain key", () => {
    const t = setup(bindings);
    holdAlt(t);
    t.engine.keydown(arrow("ArrowDown"));
    // 뗌은 더블 탭(alt alt)이 아니다 — 누른 채 친 키가 이미 실행됐다.
    expect(t.engine.keyup(altUp()).handled).toBe(false);
    // ⌥ 없이 ↓ 는 아무 바인딩도 아니다(에디터의 보통 이동).
    expect(t.engine.keydown(key("ArrowDown")).handled).toBe(false);
    // ⌥ 를 한 번만 누른 ↓ 는 보통 바인딩 alt+down 이다.
    t.engine.keydown(altDown());
    expect(t.engine.keydown(arrow("ArrowDown")).action).toBe("editor.shrinkSelection");
    expect(t.ran).toEqual(["editor.addCursorBelow", "editor.shrinkSelection"]);
  });

  it("accepts a first press and a gap of exactly the limit, and not one millisecond more", () => {
    const at = (press: number, gap: number) => {
      const t = setup(bindings);
      holdAlt(t, press, gap);
      return t.engine.keydown(arrow("ArrowDown")).action;
    };
    expect(at(300, 100)).toBe("editor.addCursorBelow");
    expect(at(301, 100)).toBe("editor.shrinkSelection");
    expect(at(50, 300)).toBe("editor.addCursorBelow");
    expect(at(50, 301)).toBe("editor.shrinkSelection");
  });

  it("is not a hold with a single press, or after another key broke the taps", () => {
    const single = setup(bindings);
    single.engine.keydown(altDown());
    single.tick(1000);
    expect(single.engine.keydown(arrow("ArrowDown")).action).toBe("editor.shrinkSelection");

    const broken = setup(bindings);
    broken.engine.keydown(altDown());
    broken.tick(30);
    broken.engine.keyup(altUp());
    broken.engine.keydown(key("KeyX"));
    broken.tick(30);
    broken.engine.keydown(altDown());
    expect(broken.engine.keydown(arrow("ArrowDown")).action).toBe("editor.shrinkSelection");
  });

  it("still runs the plain double tap when no key was pressed while holding", () => {
    const t = setup(bindings);
    holdAlt(t);
    t.tick(50);
    expect(t.engine.keyup(altUp())).toEqual({ handled: true, action: "alt.double" });
  });

  it("repeats a held key only when the action allows it", () => {
    const allowed = setup(bindings, { repeatable: ["editor.addCursorBelow"] });
    holdAlt(allowed);
    allowed.engine.keydown(arrow("ArrowDown"));
    expect(allowed.engine.keydown(arrow("ArrowDown", { repeat: true })).action).toBe(
      "editor.addCursorBelow",
    );
    expect(allowed.ran).toHaveLength(2);

    const denied = setup(bindings);
    holdAlt(denied);
    denied.engine.keydown(arrow("ArrowDown"));
    expect(denied.engine.keydown(arrow("ArrowDown", { repeat: true }))).toEqual({ handled: true });
    expect(denied.ran).toHaveLength(1);
  });

  it("is seen by peek without changing the hold", () => {
    const t = setup(bindings);
    // ⌥↑ 는 보통 바인딩이 없어 누른 채일 때만 가져간다.
    expect(t.engine.peek(arrow("ArrowUp"))).toBe(false);
    holdAlt(t);
    expect(t.engine.peek(arrow("ArrowUp"))).toBe(true);
    expect(t.engine.keydown(arrow("ArrowUp")).action).toBe("editor.addCursorAbove");
  });

  it("is not a hold when the key comes without the modifier (its keyup was lost to another window)", () => {
    const t = setup(bindings);
    holdAlt(t);
    expect(t.engine.keydown(key("ArrowUp")).handled).toBe(false);
    expect(t.ran).toEqual([]);
  });

  it("skips a disabled hold binding and falls back to the plain key", () => {
    const t = setup(bindings, { disabled: ["editor.addCursorBelow"] });
    holdAlt(t);
    expect(t.engine.keydown(arrow("ArrowDown")).action).toBe("editor.shrinkSelection");
  });
});

describe("KeymapEngine — when and priority", () => {
  const bindings = keymap(`
[[bindings]]
action = "global.f2"
key = "f2"
[[bindings]]
action = "toolWindow.f2"
key = "f2"
when = "toolWindowFocus"
[[bindings]]
action = "terminal.f2"
key = "f2"
when = "terminalFocus"
`);

  it("prefers the binding whose condition reads the innermost context", () => {
    const { engine, setContext } = setup(bindings);
    expect(engine.keydown(key("F2")).action).toBe("global.f2");
    setContext([{}, { toolWindowFocus: true }, {}]);
    expect(engine.keydown(key("F2")).action).toBe("toolWindow.f2");
    setContext([{ terminalFocus: true }, { toolWindowFocus: true }, {}]);
    expect(engine.keydown(key("F2")).action).toBe("terminal.f2");
  });

  it("breaks a tie by merge order — the later binding wins", () => {
    const base = keymap(`[[bindings]]\naction = "action.find"\nkey = "cmd+shift+a"\n`);
    const user = readUserKeymap(
      parseKeymapToml(
        `[[bindings]]\naction = "settings.open"\nkey = "cmd+shift+a"\nmode = "add"\n`,
      ),
      "mac",
    );
    const { engine } = setup(mergeKeymap(base, user));
    expect(engine.keydown(key("KeyA", cmdShift)).action).toBe("settings.open");
  });
});

describe("창 규칙(admits)과 미리 보기(peek)", () => {
  const bindings = keymap(`
schema = 1
[[bindings]]
action = "a.find"
mac = "cmd+shift+a"
[[bindings]]
action = "a.close"
mac = "cmd+w"
[[bindings]]
action = "t.close"
mac = "cmd+w"
when = "terminalFocus"
[[bindings]]
action = "a.chord"
mac = "cmd+k cmd+s"
`);
  function withRule(admits: (b: Binding, layers: ContextLayers) => boolean) {
    const ran: string[] = [];
    let layers: ContextLayers = [{ terminalFocus: true }, {}, {}];
    const engine = new KeymapEngine({
      run: (action) => {
        ran.push(action);
        return true;
      },
      isEnabled: () => true,
      context: () => layers,
      admits,
      now: () => 0,
    });
    engine.setBindings(bindings);
    return {
      engine,
      ran,
      setContext(next: ContextLayers) {
        layers = next;
      },
    };
  }

  it("거른 바인딩은 실행하지 않고 키도 가져가지 않는다 — peek 도 같은 답이다", () => {
    const { engine, ran } = withRule((b) => b.action !== "a.find");
    expect(engine.peek(key("KeyA", cmdShift))).toBe(false);
    expect(engine.keydown(key("KeyA", cmdShift)).handled).toBe(false);
    expect(ran).toEqual([]);
  });

  it("peek 은 실행하지 않고, 실행할 바인딩이 있으면 참이다", () => {
    const { engine, ran } = withRule(() => true);
    expect(engine.peek(key("KeyW", { metaKey: true }))).toBe(true);
    expect(ran).toEqual([]);
    // 컨텍스트를 읽은 바인딩(`terminalFocus`)이 이긴다.
    expect(engine.keydown(key("KeyW", { metaKey: true })).action).toBe("t.close");
  });

  it("peek 은 연속 키를 기다리는 상태를 만들지도 지우지도 않는다", () => {
    const { engine, ran } = withRule(() => true);
    expect(engine.peek(key("KeyK", { metaKey: true }))).toBe(true);
    // peek 뒤의 ⌘S 는 연속 키의 둘째 누름이 아니다.
    expect(engine.keydown(key("KeyS", { metaKey: true })).handled).toBe(false);
    expect(engine.keydown(key("KeyK", { metaKey: true })).pending).toBe(true);
    // 기다리는 중에 peek 해도 기다림은 남는다.
    expect(engine.peek(key("KeyS", { metaKey: true }))).toBe(true);
    expect(engine.keydown(key("KeyS", { metaKey: true })).action).toBe("a.chord");
    expect(ran).toEqual(["a.chord"]);
  });

  it("IME 조합 중인 키와 수정 키만 누른 것은 peek 이 거짓이다", () => {
    const { engine } = withRule(() => true);
    expect(engine.peek(key("KeyW", { metaKey: true }, { isComposing: true }))).toBe(false);
    expect(engine.peek(key("MetaLeft", { metaKey: true }))).toBe(false);
  });
});
