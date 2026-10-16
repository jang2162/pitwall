import { describe, expect, it } from "vitest";

import {
  type Binding,
  KeymapFileError,
  menuAccelerators,
  mergeKeymap,
  parseKeymapToml,
  readDefaultKeymap,
  readUserKeymap,
} from "../src/keymap-file";
import { formatSequence } from "../src/keys";

const DEFAULT = parseKeymapToml(`
schema = 1

[[bindings]]
action = "action.find"
mac = "cmd+shift+a"
win = "ctrl+shift+a"
linux = "ctrl+shift+a"

[[bindings]]
action = "settings.open"
mac = "cmd+,"
win = "ctrl+alt+s"
linux = "ctrl+alt+s"

[[bindings]]
action = "toolWindow.project"
mac = "cmd+1"
win = "alt+1"
linux = "alt+1"

[[bindings]]
action = "mac.only"
mac = "cmd+j"
`);

const show = (bindings: readonly Binding[]) =>
  bindings.map(
    (b) => `${b.action}=${formatSequence(b.sequence, "mac")}${b.when ? `@${b.when}` : ""}`,
  );

const base = readDefaultKeymap(DEFAULT, "mac");

describe("readDefaultKeymap", () => {
  it("takes each binding's key for this OS and drops bindings without one", () => {
    expect(show(base)).toEqual([
      "action.find=⇧⌘A",
      "settings.open=⌘,",
      "toolWindow.project=⌘1",
      "mac.only=⌘J",
    ]);
    expect(readDefaultKeymap(DEFAULT, "win").map((b) => b.key)).toEqual([
      "ctrl+shift+a",
      "ctrl+alt+s",
      "alt+1",
    ]);
  });

  it("checks every OS's key even on this OS", () => {
    const broken = parseKeymapToml(
      `[[bindings]]\naction = "a"\nmac = "cmd+a"\nwin = "ctrl+nope"\n`,
    );
    expect(() => readDefaultKeymap(broken, "mac")).toThrow("bindings[0].win");
  });
});

describe("readUserKeymap + mergeKeymap", () => {
  const merge = (toml: string) =>
    show(mergeKeymap(base, readUserKeymap(parseKeymapToml(toml), "mac")));

  it("is the default keymap when there is no user file", () => {
    expect(show(mergeKeymap(base, readUserKeymap(undefined, "mac")))).toEqual(show(base));
  });

  it("replaces an action's default keys", () => {
    expect(merge(`[[bindings]]\naction = "action.find"\nkey = "cmd+k"\n`)).toEqual([
      "settings.open=⌘,",
      "toolWindow.project=⌘1",
      "mac.only=⌘J",
      "action.find=⌘K",
    ]);
  });

  it("keeps several replacing keys for the same action", () => {
    expect(
      merge(
        `[[bindings]]\naction = "action.find"\nkey = "cmd+k"\n[[bindings]]\naction = "action.find"\nkey = "cmd+l"\n`,
      ),
    ).toContain("action.find=⌘L");
  });

  it("adds a key with mode = add and keeps the default", () => {
    expect(
      merge(`[[bindings]]\naction = "action.find"\nkey = "cmd+shift+space"\nmode = "add"\n`),
    ).toEqual([...show(base), "action.find=⇧⌘Space"]);
  });

  it("carries a when condition", () => {
    expect(
      merge(`[[bindings]]\naction = "settings.open"\nkey = "cmd+;"\nwhen = "editorFocus"\n`),
    ).toContain("settings.open=⌘;@editorFocus");
  });

  it("unbinds by action and key, by key alone, or by action alone", () => {
    expect(merge(`[[unbind]]\naction = "settings.open"\nkey = "cmd+,"\n`)).not.toContain(
      "settings.open=⌘,",
    );
    expect(merge(`[[unbind]]\naction = "settings.open"\nkey = "cmd+;"\n`)).toContain(
      "settings.open=⌘,",
    );
    expect(merge(`[[unbind]]\nkey = "cmd+1"\n`)).not.toContain("toolWindow.project=⌘1");
    expect(merge(`[[unbind]]\naction = "mac.only"\n`)).not.toContain("mac.only=⌘J");
  });

  it("does not unbind on this OS when the unbind names another OS's key only", () => {
    expect(merge(`[[unbind]]\naction = "mac.only"\nwin = "ctrl+j"\n`)).toContain("mac.only=⌘J");
  });

  it("puts user bindings after the defaults so they win a shared key", () => {
    const merged = merge(
      `[[bindings]]\naction = "settings.open"\nkey = "cmd+shift+a"\nmode = "add"\n`,
    );
    expect(merged.indexOf("settings.open=⇧⌘A")).toBeGreaterThan(merged.indexOf("action.find=⇧⌘A"));
  });

  it.each([
    ["bad key", `[[bindings]]\naction = "a"\nkey = "cmd+nope"\n`, "bindings[0].key"],
    ["no action", `[[bindings]]\nkey = "cmd+a"\n`, "bindings[0].action"],
    ["no key", `[[bindings]]\naction = "a"\n`, "key"],
    ["typo field", `[[bindings]]\nacton = "a"\naction = "a"\nkey = "cmd+a"\n`, "모르는 필드 acton"],
    ["bad mode", `[[bindings]]\naction = "a"\nkey = "cmd+a"\nmode = "merge"\n`, "mode"],
    ["bad when", `[[bindings]]\naction = "a"\nkey = "cmd+a"\nwhen = "a &&"\n`, "when"],
    ["empty unbind", `[[unbind]]\nwhen = "x"\n`, "action 이나 key"],
    ["bindings not an array", `bindings = "x"\n`, "배열"],
    ["unknown top-level", `binding = []\n`, "모르는 최상위 키 binding"],
    ["future schema", `schema = 2\n`, "schema"],
  ])("rejects the whole file on %s", (_, toml, message) => {
    expect(() => readUserKeymap(parseKeymapToml(toml), "mac")).toThrow(KeymapFileError);
    expect(() => readUserKeymap(parseKeymapToml(toml), "mac")).toThrow(message);
  });

  it("reports a TOML syntax error as a keymap file error", () => {
    expect(() => parseKeymapToml("[[bindings]\n")).toThrow(KeymapFileError);
  });
});

describe("menuAccelerators", () => {
  it("takes the first unconditional single-stroke key of each menu action", () => {
    const user = readUserKeymap(
      parseKeymapToml(`
[[bindings]]
action = "action.find"
key = "cmd+k cmd+a"
[[bindings]]
action = "action.find"
key = "cmd+e"
when = "editorFocus"
[[bindings]]
action = "action.find"
key = "cmd+shift+p"
`),
      "mac",
    );
    expect(
      menuAccelerators(mergeKeymap(base, user), ["action.find", "settings.open", "help.none"]),
    ).toEqual({
      "action.find": "Super+Shift+KeyP",
      "settings.open": "Super+Comma",
      "help.none": null,
    });
  });
});
