import { describe, expect, it, vi } from "vitest";

import {
  createThemeStore,
  type NativeWindowTheme,
  type PreferenceStorage,
  type SystemAppearance,
  type ThemePreference,
} from "./theme-store";
import type { Appearance } from "./tokens";

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

function fakeOs(initial: Appearance) {
  let value = initial;
  const listeners = new Set<(a: Appearance) => void>();
  const system: SystemAppearance & { change(a: Appearance): void } = {
    current: () => value,
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    change(next) {
      value = next;
      for (const listener of listeners) listener(next);
    },
  };
  return system;
}

function fakeStorage(initial?: ThemePreference) {
  let value = initial;
  const listeners = new Set<(p: ThemePreference) => void>();
  const storage: PreferenceStorage & {
    value(): ThemePreference | undefined;
    otherWindowWrites(p: ThemePreference): void;
  } = {
    read: () => value,
    write: (p) => void (value = p),
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    value: () => value,
    otherWindowWrites(p) {
      value = p;
      for (const listener of listeners) listener(p);
    },
  };
  return storage;
}

function setup(os: Appearance, saved?: ThemePreference, native?: NativeWindowTheme) {
  const system = fakeOs(os);
  const storage = fakeStorage(saved);
  const apply = vi.fn<(a: Appearance) => void>();
  const store = createThemeStore({ system, storage, native, apply });
  return { system, storage, apply, store };
}

describe("theme store", () => {
  it("applies the OS appearance synchronously at creation when nothing is saved", () => {
    const { store, apply } = setup("dark");
    expect(store.getState()).toMatchObject({ preference: "system", appearance: "dark" });
    expect(apply.mock.calls).toEqual([["dark"]]);
  });

  it("follows OS changes while the preference is system", () => {
    const { store, system, apply } = setup("light");
    system.change("dark");
    expect(store.getState().appearance).toBe("dark");
    system.change("light");
    expect(apply.mock.calls).toEqual([["light"], ["dark"], ["light"]]);
  });

  it("keeps a manual choice when the OS changes, and saves it", () => {
    const { store, system, storage, apply } = setup("light");
    store.getState().setPreference("dark");
    expect(store.getState().appearance).toBe("dark");
    expect(storage.value()).toBe("dark");
    system.change("light");
    system.change("dark");
    system.change("light");
    expect(store.getState().appearance).toBe("dark");
    expect(apply.mock.calls).toEqual([["light"], ["dark"]]);
  });

  it("returns to the current OS appearance when switched back to system", () => {
    const { store, system } = setup("light");
    store.getState().setPreference("light");
    system.change("dark");
    expect(store.getState().appearance).toBe("light");
    store.getState().setPreference("system");
    expect(store.getState().appearance).toBe("dark");
  });

  it("restores a saved manual choice over the OS appearance", () => {
    const { store, apply } = setup("light", "dark");
    expect(store.getState()).toMatchObject({ preference: "dark", appearance: "dark" });
    expect(apply.mock.calls).toEqual([["dark"]]);
  });

  it("follows a choice made in another window", () => {
    const { store, storage } = setup("light");
    storage.otherWindowWrites("dark");
    expect(store.getState()).toMatchObject({ preference: "dark", appearance: "dark" });
    storage.otherWindowWrites("system");
    expect(store.getState()).toMatchObject({ preference: "system", appearance: "light" });
  });

  it("stops listening after dispose", () => {
    const { store, system, storage } = setup("light");
    store.dispose();
    system.change("dark");
    storage.otherWindowWrites("dark");
    expect(store.getState().appearance).toBe("light");
  });
});

describe("native window theme", () => {
  function fakeNative() {
    const listeners = new Set<(a: Appearance) => void>();
    const native = {
      set: vi.fn<(t: Appearance | null) => Promise<void>>(async () => {}),
      subscribe(listener: (a: Appearance) => void) {
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
      emit(a: Appearance) {
        for (const listener of listeners) listener(a);
      },
    };
    return native;
  }

  it("pins the native window to a manual choice and unpins it for system", async () => {
    const native = fakeNative();
    const { store } = setup("light", undefined, native);
    expect(native.set).not.toHaveBeenCalled();
    store.getState().setPreference("dark");
    store.getState().setPreference("system");
    expect(native.set.mock.calls).toEqual([["dark"], [null]]);
  });

  it("pins the native window at start when a manual choice is saved", () => {
    const native = fakeNative();
    setup("light", "dark", native);
    expect(native.set.mock.calls).toEqual([["dark"]]);
  });

  it("treats the Tauri window theme event as an OS change", () => {
    const native = fakeNative();
    const { store } = setup("light", undefined, native);
    native.emit("dark");
    expect(store.getState().appearance).toBe("dark");
  });

  it("re-reads the OS appearance once the native pin is released", async () => {
    const native = fakeNative();
    const { store, system } = setup("light", undefined, native);
    store.getState().setPreference("dark");
    // 고정한 동안 OS 쪽 값이 바뀌었지만 수동 선택이라 받지 않았다.
    system.change("dark");
    store.getState().setPreference("light");
    store.getState().setPreference("system");
    await flush();
    expect(store.getState().appearance).toBe("dark");
  });

  it("keeps working when the native window refuses the theme", async () => {
    const native = fakeNative();
    native.set.mockRejectedValue(new Error("not allowed"));
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const { store } = setup("light", undefined, native);
    store.getState().setPreference("dark");
    await flush();
    expect(store.getState().appearance).toBe("dark");
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });
});
