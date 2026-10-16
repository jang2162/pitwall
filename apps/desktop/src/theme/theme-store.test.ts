import { describe, expect, it, vi } from "vitest";

import {
  createThemeStore,
  type NativeWindowTheme,
  type PreferenceStorage,
  type SystemAppearance,
  type ThemePreference,
} from "./theme-store";
import type { Appearance, Theme, TokenValues } from "./tokens";

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

// 스토어는 토큰 값을 보지 않는다. 어느 변형이 있는지만 본다.
const values = {} as TokenValues;
const bothVariants: Theme = {
  id: "both",
  label: "Both",
  variants: { light: values, dark: values },
};
const darkOnly: Theme = { id: "dusk", label: "Dusk", variants: { dark: values } };

function setup(
  os: Appearance,
  saved?: ThemePreference,
  native?: NativeWindowTheme,
  theme: Theme = bothVariants,
) {
  const system = fakeOs(os);
  const storage = fakeStorage(saved);
  const apply = vi.fn<(t: Theme, a: Appearance) => void>();
  const store = createThemeStore({ system, storage, native, theme, apply });
  /** 칠한 외관만 차례로. */
  const applied = () => apply.mock.calls.map(([, appearance]) => appearance);
  return { system, storage, apply, applied, store };
}

describe("theme store", () => {
  it("applies the OS appearance synchronously at creation when nothing is saved", () => {
    const { store, applied } = setup("dark");
    expect(store.getState()).toMatchObject({ preference: "system", appearance: "dark" });
    expect(applied()).toEqual(["dark"]);
  });

  it("follows OS changes while the preference is system", () => {
    const { store, system, applied } = setup("light");
    system.change("dark");
    expect(store.getState().appearance).toBe("dark");
    system.change("light");
    expect(applied()).toEqual(["light", "dark", "light"]);
  });

  it("keeps a manual choice when the OS changes, and saves it", () => {
    const { store, system, storage, applied } = setup("light");
    store.getState().setPreference("dark");
    expect(store.getState().appearance).toBe("dark");
    expect(storage.value()).toBe("dark");
    system.change("light");
    system.change("dark");
    system.change("light");
    expect(store.getState().appearance).toBe("dark");
    expect(applied()).toEqual(["light", "dark"]);
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
    const { store, applied } = setup("light", "dark");
    expect(store.getState()).toMatchObject({ preference: "dark", appearance: "dark" });
    expect(applied()).toEqual(["dark"]);
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

describe("a theme with one variant", () => {
  it("paints its only variant whatever is chosen, and keeps the choice", () => {
    const { store, storage, applied } = setup("light", undefined, undefined, darkOnly);
    expect(store.getState()).toMatchObject({ preference: "system", appearance: "dark" });
    store.getState().setPreference("light");
    expect(store.getState()).toMatchObject({ preference: "light", appearance: "dark" });
    expect(storage.value()).toBe("light");
    expect(applied()).toEqual(["dark"]);
  });

  it("uses the chosen appearance again once the theme has both variants", () => {
    const { store, system, apply } = setup("light", "light", undefined, darkOnly);
    expect(store.getState().appearance).toBe("dark");
    store.getState().setTheme(bothVariants);
    expect(store.getState().appearance).toBe("light");
    store.getState().setPreference("system");
    system.change("dark");
    expect(store.getState().appearance).toBe("dark");
    expect(apply.mock.calls).toEqual([
      [darkOnly, "dark"],
      [bothVariants, "light"],
      [bothVariants, "dark"],
    ]);
  });

  it("repaints when the theme changes even if the appearance does not", () => {
    const { store, apply } = setup("dark");
    store.getState().setTheme(darkOnly);
    expect(apply.mock.calls).toEqual([
      [bothVariants, "dark"],
      [darkOnly, "dark"],
    ]);
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

  it("pins the native window to the only variant of a theme, then follows the OS again", async () => {
    const native = fakeNative();
    const { store, system } = setup("light", undefined, native, darkOnly);
    expect(native.set.mock.calls).toEqual([["dark"]]);
    // 고정된 동안 OS 변화(실제로는 고정한 값의 메아리)는 받지 않는다.
    system.change("light");
    expect(store.getState().appearance).toBe("dark");
    store.getState().setTheme(bothVariants);
    expect(native.set.mock.calls).toEqual([["dark"], [null]]);
    await flush();
    expect(store.getState().appearance).toBe("light");
  });

  it("treats the Tauri window theme event as an OS change", () => {
    const native = fakeNative();
    const { store } = setup("light", undefined, native);
    native.emit("dark");
    expect(store.getState().appearance).toBe("dark");
  });

  // 웹뷰의 prefers-color-scheme 은 창 테마를 따른다. 창이 고정된 동안 system.current() 는 OS 가 아니라 고정한 값을
  // 주고, set(null) 이 끝나야 OS 값을 준다. 이 모형에서는 고정을 푼 뒤 다시 읽어야만 OS 값에 닿는다.
  it("re-reads the OS appearance once the native pin is released", async () => {
    const os: Appearance = "dark";
    let pinned: Appearance | null = null;
    const pending: (() => void)[] = [];
    const system: SystemAppearance = {
      current: () => pinned ?? os,
      subscribe: () => () => {},
    };
    const native: NativeWindowTheme = {
      set: (theme) =>
        new Promise<void>((resolve) =>
          pending.push(() => {
            pinned = theme;
            resolve();
          }),
        ),
    };
    const release = async () => {
      for (const done of pending.splice(0)) done();
      await flush();
    };
    const store = createThemeStore({
      system,
      storage: fakeStorage("light"),
      native,
      theme: bothVariants,
      apply: () => {},
    });
    await release();
    expect(store.getState().appearance).toBe("light");

    store.getState().setPreference("system");
    // set(null) 이 아직 안 끝났다 — 창은 라이트로 고정돼 있어 지금 읽으면 라이트다.
    expect(store.getState().appearance).toBe("light");
    await release();
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
