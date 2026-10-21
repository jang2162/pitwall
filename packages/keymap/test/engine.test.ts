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
