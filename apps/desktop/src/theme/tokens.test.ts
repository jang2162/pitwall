// @vitest-environment happy-dom
/// <reference types="node" />
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { gridColors, opaqueOver } from "../db/grid/grid-theme";
import { documentThemeApplier } from "./adapters";
import defaultThemeFile from "../../../../extensions/theme-default/themes/default.json";
import {
  DB_TOKENS,
  DIAGNOSTIC_TOKENS,
  DIFF_TOKENS,
  GIT_STATUS_TOKENS,
  GRAPH_LANES,
  GRAPH_TOKENS,
  graphLaneToken,
  GRID_TOKENS,
  parseTheme,
  resolveAppearance,
  themeAppearances,
  tokenCssVar,
  TOKENS,
} from "./tokens";

const defaultTheme = parseTheme(defaultThemeFile);

/** WCAG 2 의 대비(1 ~ 21). 불투명 `#rrggbb` 만 받는다 — 다른 꼴이면 조용히 통과하지 않게 던진다. */
function contrast(a: string, b: string): number {
  const luminance = (hex: string) => {
    const m = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(hex);
    if (!m) throw new Error(`not an opaque #rrggbb color: ${hex}`);
    const [r, g, b] = m.slice(1).map((c) => {
      const v = parseInt(c, 16) / 255;
      return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
    }) as [number, number, number];
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x) as [number, number];
  return (hi + 0.05) / (lo + 0.05);
}

/** 트리 · 탭의 git 글자(이름과 표지)가 놓이는 배경: 트리의 보통 · 호버 · 선택 행, 활성 · 비활성 탭. */
const GIT_TEXT_BACKGROUNDS = [
  "ui.panelBackground",
  "ui.hoverBackground",
  "ui.activeBackground",
  "tab.activeBackground",
  "tab.inactiveBackground",
] as const;

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
      expect(Object.keys(defaultTheme.variants[appearance]!).sort()).toEqual([...TOKENS].sort());
    }
  });

  it("accepts a theme with only one variant", () => {
    const theme = parseTheme({
      ...defaultThemeFile,
      variants: { dark: defaultThemeFile.variants.dark },
    });
    expect(themeAppearances(theme)).toEqual(["dark"]);
    expect(theme.variants.light).toBeUndefined();
  });

  it("rejects a theme without any variant, or with a variant that is not an object", () => {
    expect(() => parseTheme({ ...defaultThemeFile, variants: {} })).toThrow("변형이 하나도 없다");
    expect(() => parseTheme({ ...defaultThemeFile, variants: { light: "#fff" } })).toThrow(
      "light 변형이 객체가 아니다",
    );
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

describe("git · graph · diff tokens (M2-12)", () => {
  const added = [...GIT_STATUS_TOKENS, ...GRAPH_TOKENS, ...DIFF_TOKENS];

  it("are tokens the app knows, and the default theme defines each in both variants", () => {
    expect(added.filter((token) => !(TOKENS as readonly string[]).includes(token))).toEqual([]);
    for (const appearance of ["light", "dark"] as const) {
      const file: Record<string, unknown> = defaultThemeFile.variants[appearance];
      expect(added.filter((token) => typeof file[token] !== "string")).toEqual([]);
    }
  });

  it("give every git status and every graph lane its own color in each variant", () => {
    for (const appearance of ["light", "dark"] as const) {
      const values = defaultTheme.variants[appearance]!;
      const git = GIT_STATUS_TOKENS.map((t) => values[t].toLowerCase());
      expect(new Set(git).size).toBe(GIT_STATUS_TOKENS.length);
      const lanes = GRAPH_TOKENS.filter((t) => /^graph\.lane\.\d+$/.test(t)).map((t) =>
        values[t].toLowerCase(),
      );
      expect(new Set(lanes).size).toBe(GRAPH_LANES);
    }
  });

  it("keep the folded lane apart from the cycled lanes and visible with room (3.5:1) on the panel background", () => {
    for (const appearance of ["light", "dark"] as const) {
      const values = defaultTheme.variants[appearance]!;
      const folded = values["graph.lane.folded"].toLowerCase();
      const lanes = Array.from({ length: GRAPH_LANES }, (_, i) =>
        values[graphLaneToken(i)].toLowerCase(),
      );
      expect(lanes).not.toContain(folded);
      expect(contrast(folded, values["ui.panelBackground"])).toBeGreaterThanOrEqual(3.5);
    }
  });

  it("keep every git status at body-text contrast (4.5:1) on the tree and tab backgrounds, ignored no lower than the muted color it replaced", () => {
    expect(contrast("#000000", "#ffffff")).toBeCloseTo(21);
    expect(contrast("#777777", "#777777")).toBe(1);
    const short: string[] = [];
    for (const appearance of ["light", "dark"] as const) {
      const values = defaultTheme.variants[appearance]!;
      for (const background of GIT_TEXT_BACKGROUNDS)
        for (const token of GIT_STATUS_TOKENS) {
          const floor =
            token === "git.ignored"
              ? Math.max(4.5, contrast(values["ui.mutedForeground"], values[background]))
              : 4.5;
          const ratio = contrast(values[token], values[background]);
          if (ratio < floor)
            short.push(
              `${appearance} ${token} on ${background}: ${ratio.toFixed(2)} < ${floor.toFixed(2)}`,
            );
        }
    }
    expect(short).toEqual([]);
  });

  it("keep every DB mark at body-text contrast (4.5:1) where its label is drawn", () => {
    // 연결 표지의 라벨(「프로덕션」 · 「읽기 전용」) · TLS 표지 · 태그 이름이 놓이는 곳: 탐색기의 보통 · 호버 · 선택 행, 위 막대,
    // 다이얼로그, 탭, 상태바.
    const backgrounds = [
      "ui.panelBackground",
      "ui.hoverBackground",
      "ui.activeBackground",
      "ui.background",
      "ui.statusBarBackground",
      "tab.activeBackground",
      "tab.inactiveBackground",
      "editor.background",
    ] as const;
    const short: string[] = [];
    for (const appearance of ["light", "dark"] as const) {
      const values = defaultTheme.variants[appearance]!;
      for (const background of backgrounds)
        for (const token of DB_TOKENS) {
          const ratio = contrast(values[token], values[background]);
          if (ratio < 4.5)
            short.push(`${appearance} ${token} on ${background}: ${ratio.toFixed(2)} < 4.50`);
        }
    }
    expect(short).toEqual([]);
  });

  it("keep popup text at body-text contrast (4.5:1) on the popup, its code blocks and, for the main text, the selected row", () => {
    const pairs = [
      ["popup.foreground", "popup.background"],
      ["popup.foreground", "popup.codeBackground"],
      ["popup.foreground", "popup.selectionBackground"],
      ["popup.mutedForeground", "popup.background"],
      ["popup.mutedForeground", "popup.codeBackground"],
    ] as const;
    const short: string[] = [];
    for (const appearance of ["light", "dark"] as const) {
      const values = defaultTheme.variants[appearance]!;
      for (const [text, background] of pairs) {
        const ratio = contrast(values[text], values[background]);
        if (ratio < 4.5)
          short.push(`${appearance} ${text} on ${background}: ${ratio.toFixed(2)} < 4.50`);
      }
    }
    expect(short).toEqual([]);
  });

  it("keep diagnostic underlines and markers at graphics contrast (3:1) on the editor, tool windows, status bar, popups and their rows", () => {
    const backgrounds = [
      "editor.background",
      "ui.panelBackground",
      "ui.statusBarBackground",
      "ui.hoverBackground",
      "ui.activeBackground",
      "popup.background",
    ] as const;
    const short: string[] = [];
    for (const appearance of ["light", "dark"] as const) {
      const values = defaultTheme.variants[appearance]!;
      for (const background of backgrounds)
        for (const token of DIAGNOSTIC_TOKENS) {
          const ratio = contrast(values[token], values[background]);
          if (ratio < 3)
            short.push(`${appearance} ${token} on ${background}: ${ratio.toFixed(2)} < 3.00`);
        }
    }
    expect(short).toEqual([]);
  });

  it("cycle graph lanes through graph.lane.1..GRAPH_LANES", () => {
    expect(GRAPH_TOKENS.filter((t) => /^graph\.lane\.\d+$/.test(t))).toHaveLength(GRAPH_LANES);
    expect(graphLaneToken(0)).toBe("graph.lane.1");
    expect(graphLaneToken(GRAPH_LANES - 1)).toBe(`graph.lane.${GRAPH_LANES}`);
    expect(graphLaneToken(GRAPH_LANES)).toBe("graph.lane.1");
    expect(graphLaneToken(63)).toBe(`graph.lane.${(63 % GRAPH_LANES) + 1}`);
  });

  it("turn into CSS variables the screens read", () => {
    expect(tokenCssVar("git.conflicted")).toBe("--pw-git-conflicted");
    expect(tokenCssVar("graph.lane.8")).toBe("--pw-graph-lane-8");
    expect(tokenCssVar("diff.char.added")).toBe("--pw-diff-char-added");
  });
});

describe("grid tokens (M3-16)", () => {
  // 결과 그리드(canvas)에서 글자가 놓이는 자리: 셀 글자 · NULL · 행 번호(Glide `textLight` = grid.null)는 보통 · 고른 셀 위,
  // 열 머리의 이름 · 타입 · 정렬 화살표는 보통 · hover(고른 셀이 있는 열 머리 포함) 위, 고른 열 머리의 글자는 accent 위.
  const TEXT_ON: Record<string, readonly string[]> = {
    "grid.foreground": ["grid.background", "grid.selection"],
    "grid.null": ["grid.background", "grid.selection"],
    "grid.headerForeground": ["grid.headerBackground", "grid.headerHover"],
    "grid.headerType": ["grid.headerBackground", "grid.headerHover"],
    "grid.accent": ["grid.headerBackground", "grid.headerHover"],
    "grid.background": ["grid.accent"],
  };
  // 글자를 그리지 않는 선(셀 · 머리 경계).
  const LINES = ["grid.border"];

  it("place every grid token: as text over its backgrounds, as a background, or as a line", () => {
    const placed = new Set([...Object.keys(TEXT_ON), ...Object.values(TEXT_ON).flat(), ...LINES]);
    expect([...GRID_TOKENS].filter((t) => !placed.has(t))).toEqual([]);
    expect([...placed].filter((t) => !(GRID_TOKENS as readonly string[]).includes(t))).toEqual([]);
  });

  it("keep every grid text at body-text contrast (4.5:1) on light, dark, selected and hovered backgrounds", () => {
    const short: string[] = [];
    for (const appearance of ["light", "dark"] as const) {
      const values = defaultTheme.variants[appearance]!;
      for (const [token, backgrounds] of Object.entries(TEXT_ON))
        for (const background of backgrounds) {
          const ratio = contrast(
            values[token as (typeof GRID_TOKENS)[number]],
            values[background as (typeof GRID_TOKENS)[number]],
          );
          if (ratio < 4.5)
            short.push(`${appearance} ${token} on ${background}: ${ratio.toFixed(2)} < 4.50`);
        }
    }
    expect(short).toEqual([]);
  });
});

describe("grid edit marks (M3-17)", () => {
  // 테이블 데이터의 편집 대기 셀(바뀐 셀 · 추가 행 · 지울 행)은 diff 토큰을 그리드 배경 위에 섞은 색(grid-theme.ts `opaqueOver`)을
  // 배경으로 두고 그 위에 셀 글자 · NULL · 표지 글자를 그린다 — 새 토큰 없이 기존 토큰을 읽으므로 그 조합을 여기서 잰다.
  const MARKS = [
    ["diff.line.changed", "changed"],
    ["diff.line.added", "inserted"],
    ["diff.line.deleted", "deleted"],
  ] as const;

  it("keep cell text and NULL at body-text contrast (4.5:1) on every mark background", () => {
    const short: string[] = [];
    for (const appearance of ["light", "dark"] as const) {
      const values = defaultTheme.variants[appearance]!;
      const marks = gridColors(values).marks;
      for (const [mark, kind] of MARKS) {
        // 그리드가 칠하는 색 그대로(반투명이면 `contrast` 가 던진다).
        const background = marks[kind];
        for (const token of ["grid.foreground", "grid.null"] as const) {
          const ratio = contrast(values[token], background);
          if (ratio < 4.5)
            short.push(`${appearance} ${token} on ${mark}: ${ratio.toFixed(2)} < 4.50`);
        }
      }
    }
    expect(short).toEqual([]);
  });

  it("mixes a translucent mark over the grid background into an opaque color", () => {
    expect(opaqueOver("#ff000080", "#000000")).toBe("#800000");
    expect(opaqueOver("#123456", "#ffffff")).toBe("#123456");
  });
});

describe("resolveAppearance", () => {
  it("keeps the wanted appearance when the theme has it, otherwise uses the theme's only variant", () => {
    const darkOnly = parseTheme({
      ...defaultThemeFile,
      variants: { dark: defaultThemeFile.variants.dark },
    });
    const lightOnly = parseTheme({
      ...defaultThemeFile,
      variants: { light: defaultThemeFile.variants.light },
    });
    expect(resolveAppearance(defaultTheme, "light")).toBe("light");
    expect(resolveAppearance(defaultTheme, "dark")).toBe("dark");
    expect(resolveAppearance(darkOnly, "light")).toBe("dark");
    expect(resolveAppearance(lightOnly, "dark")).toBe("light");
  });
});

describe("documentThemeApplier", () => {
  it("injects every token of the variant as a CSS variable on the root", () => {
    const root = document.createElement("html");
    const apply = documentThemeApplier(root);

    apply(defaultTheme, "dark");
    expect(root.style.getPropertyValue("--pw-ui-background")).toBe(
      defaultThemeFile.variants.dark["ui.background"],
    );
    for (const token of TOKENS) {
      expect(root.style.getPropertyValue(tokenCssVar(token))).toBe(
        defaultTheme.variants.dark![token],
      );
    }
    expect(root.dataset.theme).toBe("dark");
    expect(root.style.colorScheme).toBe("dark");

    apply(defaultTheme, "light");
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

  it("maps every git status token to a Tailwind color (`text-git-<status>`)", () => {
    for (const token of GIT_STATUS_TOKENS) {
      const status = token.slice("git.".length);
      expect(styles).toContain(`--color-git-${status}: var(${tokenCssVar(token)});`);
    }
  });

  it("maps the DB marks to Tailwind colors (`text-db-production` · `-read-only` · `-insecure`)", () => {
    for (const [name, token] of [
      ["production", "db.production"],
      ["read-only", "db.readOnly"],
      ["insecure", "db.insecure"],
    ] as const)
      expect(styles).toContain(`--color-db-${name}: var(${tokenCssVar(token)});`);
  });
});
