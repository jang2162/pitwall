import {
  formatSequence,
  parseKeymapToml,
  type Platform,
  readDefaultKeymap,
  readDefaultKeymapWithWarnings,
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
    const { bindings, warnings } = readDefaultKeymapWithWarnings(
      parseKeymapToml(DEFAULT_KEYMAP_TOML),
      p,
    );
    expect(warnings).toEqual([]);
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
    const none = { bindings: base, warnings: [], good: { value: null } };
    expect(resolveKeymap(base, view(undefined), undefined, "mac")).toEqual(none);
    expect(resolveKeymap(base, undefined, undefined, "mac")).toEqual({
      bindings: base,
      warnings: [],
    });
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
      { base, bindings: previous },
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

  it("reports the TOML error, not the older snapshot's format error, when both are there", () => {
    const resolved = resolveKeymap(
      base,
      view(
        { bindings: [{ action: "action.find", key: "cmd+nope" }] },
        { message: "expected `]`", path: "/cfg/pitwall/keymap.toml" },
      ),
      { base, bindings: base },
      "mac",
    );
    expect(resolved).toEqual({ bindings: base, error: "expected `]`", warnings: [] });
  });

  it("re-merges the last good user keymap when the default keymap changes under a malformed file", () => {
    const value = { bindings: [{ action: "action.find", key: "cmd+k" }] };
    const previous = resolveKeymap(base, view(value), undefined, "mac").bindings;
    // 확장 레지스트리에서 온 새 기본 키맵: 설정 열기를 ⌘J 로 둔다.
    const registryBase = readDefaultKeymap(
      parseKeymapToml(DEFAULT_KEYMAP_TOML.replace('mac = "cmd+,"', 'mac = "cmd+j"')),
      "mac",
    );
    expect(registryBase.find((b) => b.action === "settings.open")?.key).toBe("cmd+j");
    const malformed = view({ bindings: [{ action: "action.find", key: "cmd+nope" }] });
    const resolved = resolveKeymap(registryBase, malformed, { base, bindings: previous }, "mac", {
      value,
    });
    expect(find(resolved.bindings)).toEqual(["cmd+k"]);
    expect(resolved.bindings).toEqual(
      resolveKeymap(registryBase, view(value), undefined, "mac").bindings,
    );
    expect(resolved.error).toContain("cmd+nope");
    // 마지막 성공값도 없으면 새 기본 키맵 그대로다.
    expect(
      resolveKeymap(registryBase, malformed, { base, bindings: previous }, "mac").bindings,
    ).toBe(registryBase);
  });

  it("falls back to the app-wide last good keymap in a window that has none yet", () => {
    const lastGood = { value: { bindings: [{ action: "action.find", key: "cmd+k" }] } };
    const malformed = resolveKeymap(
      base,
      view({ bindings: [{ action: "action.find", key: "cmd+nope" }] }),
      undefined,
      "mac",
      lastGood,
    );
    expect(find(malformed.bindings)).toEqual(["cmd+k"]);
    expect(malformed.good).toBeUndefined();
    // 띄울 때부터 TOML 이 틀렸으면 설정 계층의 스냅숏에는 keymap 이 없다.
    const broken = resolveKeymap(
      base,
      view(undefined, { message: "expected `]`", path: "/cfg/pitwall/keymap.toml" }),
      undefined,
      "mac",
      lastGood,
    );
    expect(find(broken.bindings)).toEqual(["cmd+k"]);
  });

  it("keeps the last good keymap before the config arrives, and does not hand back a value", () => {
    const lastGood = { value: { bindings: [{ action: "action.find", key: "cmd+k" }] } };
    const early = resolveKeymap(base, undefined, undefined, "mac", lastGood);
    expect(find(early.bindings)).toEqual(["cmd+k"]);
    expect(early.good).toBeUndefined();
  });

  it("hands back what it read so it can become the last good keymap", () => {
    const value = { bindings: [{ action: "action.find", key: "cmd+k" }] };
    expect(resolveKeymap(base, view(value), undefined, "mac").good).toEqual({ value });
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
