// @vitest-environment happy-dom
/// <reference types="node" />
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { documentThemeApplier } from "./adapters";
import defaultThemeFile from "./default-theme.json";
import { parseTheme, tokenCssVar, TOKENS } from "./tokens";

const defaultTheme = parseTheme(defaultThemeFile);

describe("tokenCssVar", () => {
  it("turns a dotted token name into a --pw- custom property", () => {
    expect(tokenCssVar("ui.background")).toBe("--pw-ui-background");
    expect(tokenCssVar("graph.lane.1")).toBe("--pw-graph-lane-1");
    expect(tokenCssVar("ui.mutedForeground")).toBe("--pw-ui-mutedForeground");
  });

  it("rejects names that could collide once dots become dashes", () => {
    expect(() => tokenCssVar("ui.muted-foreground")).toThrow();
    expect(() => tokenCssVar("background")).toThrow();
    expect(() => tokenCssVar("ui..background")).toThrow();
  });
});

describe("parseTheme", () => {
  it("accepts the default theme with both variants complete", () => {
    expect(defaultTheme.id).toBe("pitwall-default");
    for (const appearance of ["light", "dark"] as const) {
      expect(Object.keys(defaultTheme.variants[appearance]).sort()).toEqual([...TOKENS].sort());
    }
  });

  it("rejects a variant that misses a token", () => {
    const { "ui.border": _, ...rest } = defaultThemeFile.variants.dark;
    expect(() =>
      parseTheme({ ...defaultThemeFile, variants: { ...defaultThemeFile.variants, dark: rest } }),
    ).toThrow("dark.ui.border");
  });

  it("drops tokens this app does not know", () => {
    const theme = parseTheme({
      ...defaultThemeFile,
      variants: {
        ...defaultThemeFile.variants,
        light: { ...defaultThemeFile.variants.light, "future.token": "red" },
      },
    });
    expect(theme.variants.light).not.toHaveProperty("future.token");
  });
});

describe("documentThemeApplier", () => {
  it("injects every token of the variant as a CSS variable on the root", () => {
    const root = document.createElement("html");
    const apply = documentThemeApplier(root, defaultTheme);

    apply("dark");
    expect(root.style.getPropertyValue("--pw-ui-background")).toBe(
      defaultThemeFile.variants.dark["ui.background"],
    );
    for (const token of TOKENS) {
      expect(root.style.getPropertyValue(tokenCssVar(token))).toBe(
        defaultTheme.variants.dark[token],
      );
    }
    expect(root.dataset.theme).toBe("dark");
    expect(root.style.colorScheme).toBe("dark");

    apply("light");
    expect(root.style.getPropertyValue("--pw-editor-background")).toBe(
      defaultThemeFile.variants.light["editor.background"],
    );
    expect(root.dataset.theme).toBe("light");
  });
});

describe("styles.css", () => {
  // vitest 는 CSS 를 처리하지 않아 `?raw` 로 불러도 빈 문자열이 온다. 파일을 직접 읽는다. happy-dom 의 URL 은 fs 가
  // 받지 않으므로 경로 문자열로 만든다.
  const styles = readFileSync(
    join(dirname(fileURLToPath(import.meta.url)), "../styles.css"),
    "utf8",
  );

  it("reads only CSS variables that the token set defines", () => {
    const used = new Set([...styles.matchAll(/var\(\s*(--pw-[A-Za-z0-9-]+)/g)].map((m) => m[1]));
    const defined = new Set(TOKENS.map(tokenCssVar));
    expect(used.size).toBeGreaterThan(0);
    expect([...used].filter((name) => !defined.has(name!))).toEqual([]);
  });
});
