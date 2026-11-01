// 멀티 커서의 기본 키(10-ui §4.2 「멀티 커서」, 15-m4-plan M4-14)가 플랫폼마다 표의 키인지 본다. 겹침은 keymap/default-keymap.test.ts.
import { formatSequence, type Platform } from "@pitwall/keymap";
import { describe, expect, it } from "vitest";

import { chooseDefaultKeymap } from "../keymap/default-keymap";

const label = (platform: Platform, action: string) =>
  chooseDefaultKeymap(undefined, platform)
    .bindings.filter((b) => b.action === action)
    .map((b) => `${formatSequence(b.sequence, platform)} [${b.when ?? ""}]`);

describe("multi-cursor default keys", () => {
  it.each([
    ["editor.selectNextOccurrence", "⌃G", "Alt+J"],
    ["editor.unselectOccurrence", "⌃⇧G", "Alt+Shift+J"],
    ["editor.selectAllOccurrences", "⌃⌘G", "Ctrl+Alt+Shift+J"],
    ["editor.addCursorAbove", "⌥⌥↑", "Ctrl Ctrl+↑"],
    ["editor.addCursorBelow", "⌥⌥↓", "Ctrl Ctrl+↓"],
    ["editor.toggleColumnMode", "⇧⌘8", "Alt+Shift+Insert"],
    ["editor.singleCursor", "⎋", "Esc"],
  ])("binds %s to the 10-ui key in the editor text", (action, mac, other) => {
    expect(label("mac", action)).toEqual([`${mac} [editorFocus]`]);
    expect(label("win", action)).toEqual([`${other} [editorFocus]`]);
    expect(label("linux", action)).toEqual([`${other} [editorFocus]`]);
  });
});
