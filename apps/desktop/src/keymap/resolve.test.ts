import {
  formatSequence,
  parseKeymapToml,
  type Platform,
  readDefaultKeymap,
  reservedReason,
} from "@pitwall/keymap";
import type { ConfigView } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import { appActions, toolWindowActions } from "./actions";
import { DEFAULT_KEYMAP_TOML } from "./default-keymap";
import { resolveKeymap } from "./resolve";

const PLATFORMS: Platform[] = ["mac", "win", "linux"];
const defaults = (platform: Platform) =>
  readDefaultKeymap(parseKeymapToml(DEFAULT_KEYMAP_TOML), platform);

const ACTION_IDS = new Set(
  [
    ...appActions({ openFindAction() {}, openSettings: async () => {}, setTheme() {} }),
    ...toolWindowActions(() => undefined),
  ].map((a) => a.id),
);

describe("default keymap file", () => {
  it.each(PLATFORMS)("reads on %s, binds only actions that exist, and no OS-reserved key", (p) => {
    const bindings = defaults(p);
    expect(bindings.length).toBeGreaterThan(0);
    for (const b of bindings) {
      expect(ACTION_IDS, b.action).toContain(b.action);
      expect(reservedReason(b.sequence, p), `${b.action} ${b.key}`).toBeUndefined();
    }
  });

  it("follows the IntelliJ keys of 10-ui §4.2", () => {
    const label = (p: Platform, action: string) =>
      defaults(p)
        .filter((b) => b.action === action)
        .map((b) => formatSequence(b.sequence, p));
    expect(label("mac", "action.find")).toEqual(["⇧⌘A"]);
    expect(label("win", "action.find")).toEqual(["Ctrl+Shift+A"]);
    expect(label("mac", "toolWindow.project")).toEqual(["⌘1"]);
    expect(label("linux", "toolWindow.project")).toEqual(["Alt+1"]);
    expect(label("mac", "toolWindow.terminal")).toEqual(["⌥F12"]);
    expect(label("mac", "settings.open")).toEqual(["⌘,"]);
    expect(label("win", "settings.open")).toEqual(["Ctrl+Alt+S"]);
  });
});

function view(keymap: unknown, error: ConfigView["error"] = null): ConfigView {
  return {
    snapshot: {
      files: keymap === undefined ? [] : [{ id: "keymap", value: keymap as never, layers: [] }],
      notices: [],
    },
    error,
  };
}

describe("resolveKeymap", () => {
  const base = defaults("mac");
  const find = (bindings: readonly { action: string; key: string }[]) =>
    bindings.filter((b) => b.action === "action.find").map((b) => b.key);

  it("is the default keymap without a user file", () => {
    expect(resolveKeymap(base, view(undefined), undefined, "mac")).toEqual({ bindings: base });
    expect(resolveKeymap(base, undefined, undefined, "mac")).toEqual({ bindings: base });
  });

  it("applies the user's keymap.toml", () => {
    const resolved = resolveKeymap(
      base,
      view({ bindings: [{ action: "action.find", key: "cmd+k" }] }),
      undefined,
      "mac",
    );
    expect(find(resolved.bindings)).toEqual(["cmd+k"]);
    expect(resolved.error).toBeUndefined();
  });

  it("keeps the previous keymap and reports a malformed user file", () => {
    const previous = resolveKeymap(
      base,
      view({ bindings: [{ action: "action.find", key: "cmd+k" }] }),
      undefined,
      "mac",
    ).bindings;
    const resolved = resolveKeymap(
      base,
      view({ bindings: [{ action: "action.find", key: "cmd+nope" }] }),
      previous,
      "mac",
    );
    expect(resolved.bindings).toBe(previous);
    expect(resolved.error).toContain("cmd+nope");
  });

  it("reports a TOML error in keymap.toml while using the snapshot the config layer kept", () => {
    const resolved = resolveKeymap(
      base,
      view(
        { bindings: [{ action: "action.find", key: "cmd+k" }] },
        { message: "expected `]`", path: "/cfg/pitwall/keymap.toml" },
      ),
      undefined,
      "mac",
    );
    expect(find(resolved.bindings)).toEqual(["cmd+k"]);
    expect(resolved.error).toBe("expected `]`");
  });

  it("does not blame the keymap for another config file's error", () => {
    const resolved = resolveKeymap(
      base,
      view(undefined, { message: "bad", path: "/cfg/pitwall/settings.toml" }),
      undefined,
      "mac",
    );
    expect(resolved.error).toBeUndefined();
  });
});
