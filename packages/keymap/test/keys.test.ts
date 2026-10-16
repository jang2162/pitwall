import { describe, expect, it } from "vitest";

import {
  formatSequence,
  KeyParseError,
  parseKeySequence,
  reservedReason,
  sameSequence,
  strokeFromEvent,
  toAccelerator,
} from "../src/keys";

describe("parseKeySequence", () => {
  it("reads modifiers and the key as a physical code", () => {
    expect(parseKeySequence("cmd+shift+a")).toEqual({
      kind: "strokes",
      strokes: [{ cmd: true, ctrl: false, alt: false, shift: true, code: "KeyA" }],
    });
    const seq = parseKeySequence("Alt+F12");
    expect(seq.kind === "strokes" && seq.strokes[0]).toMatchObject({ alt: true, code: "F12" });
    expect(parseKeySequence("cmd+1")).toMatchObject({ strokes: [{ code: "Digit1" }] });
    expect(parseKeySequence("cmd+,")).toMatchObject({ strokes: [{ code: "Comma" }] });
    expect(parseKeySequence("ctrl+enter")).toMatchObject({ strokes: [{ code: "Enter" }] });
  });

  it("reads a double tap and a chord", () => {
    expect(parseKeySequence("shift shift")).toEqual({ kind: "double", modifier: "shift" });
    const chord = parseKeySequence("cmd+k cmd+s");
    expect(chord.kind === "strokes" && chord.strokes.map((s) => s.code)).toEqual(["KeyK", "KeyS"]);
  });

  it.each([
    ["", "비어 있다"],
    ["cmd+", "빈 자리"],
    ["cmd+shift", "수정 키만"],
    ["cmd+a+b", "둘 이상"],
    ["cmd+cmd+a", "두 번"],
    ["shift alt", "더블 탭"],
    ["shift", "더블 탭"],
    ["cmd+click", "마우스"],
    ["cmd+ㅁ", "모르는 키"],
    ["hyper+a", "모르는 키"],
  ])("rejects %j", (text, reason) => {
    expect(() => parseKeySequence(text)).toThrow(KeyParseError);
    expect(() => parseKeySequence(text)).toThrow(reason);
  });

  it("compares by keys, not by spelling", () => {
    expect(sameSequence(parseKeySequence("cmd+shift+a"), parseKeySequence("Shift+Cmd+A"))).toBe(
      true,
    );
    expect(sameSequence(parseKeySequence("cmd+a"), parseKeySequence("cmd+shift+a"))).toBe(false);
    expect(sameSequence(parseKeySequence("shift shift"), parseKeySequence("alt alt"))).toBe(false);
  });
});

describe("strokeFromEvent", () => {
  it("takes the physical code even when the IME turns the key into hangul", () => {
    const event = {
      code: "KeyA",
      key: "ㅁ",
      metaKey: true,
      ctrlKey: false,
      altKey: false,
      shiftKey: true,
    };
    expect(
      sameSequence(
        { kind: "strokes", strokes: [strokeFromEvent(event)!] },
        parseKeySequence("cmd+shift+a"),
      ),
    ).toBe(true);
  });

  it("ignores a keydown of a modifier alone", () => {
    expect(
      strokeFromEvent({
        code: "ShiftLeft",
        metaKey: false,
        ctrlKey: false,
        altKey: false,
        shiftKey: true,
      }),
    ).toBeUndefined();
  });
});

describe("formatSequence", () => {
  it("uses the macOS symbols in Apple's modifier order", () => {
    expect(formatSequence(parseKeySequence("cmd+shift+a"), "mac")).toBe("⇧⌘A");
    expect(formatSequence(parseKeySequence("alt+f12"), "mac")).toBe("⌥F12");
    expect(formatSequence(parseKeySequence("cmd+,"), "mac")).toBe("⌘,");
    expect(formatSequence(parseKeySequence("shift shift"), "mac")).toBe("⇧⇧");
    expect(formatSequence(parseKeySequence("cmd+k cmd+s"), "mac")).toBe("⌘K, ⌘S");
  });

  it("spells out modifiers elsewhere", () => {
    expect(formatSequence(parseKeySequence("ctrl+shift+a"), "win")).toBe("Ctrl+Shift+A");
    expect(formatSequence(parseKeySequence("ctrl+alt+s"), "linux")).toBe("Ctrl+Alt+S");
    expect(formatSequence(parseKeySequence("shift shift"), "win")).toBe("Shift Shift");
    expect(formatSequence(parseKeySequence("cmd+e"), "win")).toBe("Win+E");
  });

  it("labels the key with the layout's character when a layout map is given", () => {
    const azerty = new Map([
      ["KeyQ", "a"],
      ["KeyA", "q"],
    ]);
    expect(formatSequence(parseKeySequence("cmd+q"), "mac", azerty)).toBe("⌘A");
  });
});

describe("toAccelerator", () => {
  it("writes muda's accelerator from the stroke", () => {
    expect(toAccelerator(parseKeySequence("cmd+shift+a"))).toBe("Super+Shift+KeyA");
    expect(toAccelerator(parseKeySequence("ctrl+alt+s"))).toBe("Control+Alt+KeyS");
    expect(toAccelerator(parseKeySequence("alt+f12"))).toBe("Alt+F12");
  });

  it("has nothing for chords and double taps", () => {
    expect(toAccelerator(parseKeySequence("cmd+k cmd+s"))).toBeUndefined();
    expect(toAccelerator(parseKeySequence("shift shift"))).toBeUndefined();
  });
});

describe("reservedReason", () => {
  it("names the OS keys an app never receives", () => {
    expect(reservedReason(parseKeySequence("cmd+q"), "mac")).toBe("앱 종료");
    expect(reservedReason(parseKeySequence("alt+f4"), "win")).toBe("창 닫기");
    expect(reservedReason(parseKeySequence("cmd+q"), "win")).toBeUndefined();
    expect(reservedReason(parseKeySequence("cmd+shift+a"), "mac")).toBeUndefined();
  });
});
