// @vitest-environment happy-dom
/// <reference types="node" />
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it, vi } from "vitest";

import { THEME_STORAGE_KEY } from "./adapters";

// index.html 이 번들보다 먼저 부르는 스크립트(public/theme-boot.js)를 그대로 돌린다.
const source = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "../../public/theme-boot.js"),
  "utf8",
);

function boot(osDark: boolean) {
  vi.spyOn(window, "matchMedia").mockImplementation(
    (query) => ({ matches: osDark && query.includes("dark"), media: query }) as MediaQueryList,
  );
  new Function(source)();
  const root = document.documentElement;
  return { theme: root.dataset.theme, colorScheme: root.style.colorScheme };
}

afterEach(() => {
  vi.restoreAllMocks();
  localStorage.clear();
  document.documentElement.removeAttribute("data-theme");
  document.documentElement.style.colorScheme = "";
});

describe("theme-boot.js", () => {
  it("applies a saved manual choice over the OS", () => {
    localStorage.setItem(THEME_STORAGE_KEY, "dark");
    expect(boot(false)).toEqual({ theme: "dark", colorScheme: "dark" });
    localStorage.setItem(THEME_STORAGE_KEY, "light");
    expect(boot(true)).toEqual({ theme: "light", colorScheme: "light" });
  });

  it("follows the OS for system or an unreadable value", () => {
    localStorage.setItem(THEME_STORAGE_KEY, "system");
    expect(boot(true)).toEqual({ theme: "dark", colorScheme: "dark" });
    localStorage.setItem(THEME_STORAGE_KEY, "sepia");
    expect(boot(false)).toEqual({ theme: "light", colorScheme: "light" });
  });
});
