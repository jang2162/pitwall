import type { RpcClient } from "@pitwall/rpc-client";
import { describe, expect, it, vi } from "vitest";

import defaultThemeFile from "../../../../extensions/theme-default/themes/default.json";
import { applyRegistryTheme, DEFAULT_THEME_ID } from "./registry-theme";
import { createThemeStore } from "./theme-store";
import { type Appearance, parseTheme, type Theme } from "./tokens";

const bundled = parseTheme(defaultThemeFile);

function setup() {
  const apply = vi.fn<(t: Theme, a: Appearance) => void>();
  const store = createThemeStore({
    system: { current: () => "light", subscribe: () => () => {} },
    storage: { read: () => undefined, write: () => {} },
    theme: bundled,
    apply,
  });
  const log = { info: vi.fn(), warn: vi.fn() };
  return { apply, store, log };
}

function connection(reply: () => Promise<unknown>) {
  const call = vi.fn(reply);
  const close = vi.fn();
  // 테마 모듈이 부르는 것은 `call` 하나다.
  const client = { call } as unknown as Pick<RpcClient, "call">;
  return { call, close, connect: async () => ({ client, close }) };
}

describe("applyRegistryTheme", () => {
  it("replaces the bundled theme with the registry's default theme", async () => {
    const { store, apply, log } = setup();
    const registryFile = { ...defaultThemeFile, label: "From registry" };
    const conn = connection(async () => ({
      id: DEFAULT_THEME_ID,
      extension: "dev.pitwall.theme-default",
      content: registryFile,
    }));

    await applyRegistryTheme(store, conn.connect, log);

    expect(conn.call).toHaveBeenCalledWith("theme.get", { id: DEFAULT_THEME_ID });
    expect(store.getState().theme.label).toBe("From registry");
    expect(apply.mock.calls.map(([t, a]) => [t.label, a])).toEqual([
      ["Pitwall", "light"],
      ["From registry", "light"],
    ]);
    expect(log.info).toHaveBeenCalledWith(
      "theme pitwall-default applied from extension dev.pitwall.theme-default",
      "theme",
    );
    expect(conn.close).toHaveBeenCalled();
  });

  it("keeps the bundled theme when the registry fails or sends a broken theme", async () => {
    for (const reply of [
      async () => {
        throw new Error("no host");
      },
      async () => ({
        id: DEFAULT_THEME_ID,
        extension: "x",
        content: { id: DEFAULT_THEME_ID, label: "x", variants: {} },
      }),
      async () => ({
        id: DEFAULT_THEME_ID,
        extension: "x",
        content: { ...defaultThemeFile, id: "other" },
      }),
    ]) {
      const { store, log } = setup();
      const conn = connection(reply);
      await applyRegistryTheme(store, conn.connect, log);
      expect(store.getState().theme).toBe(bundled);
      expect(log.warn).toHaveBeenCalledOnce();
      expect(conn.close).toHaveBeenCalled();
    }
  });
});
