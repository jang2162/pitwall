// 빠진 번역을 잡는다: 내장 언어(ko · en)가 같은 메시지 집합을 갖는지, 화면 코드(strings.ts · app-strings.ts)와 네이티브 메뉴
// (src-tauri/src/menu.rs)가 쓰는 id 가 두 언어에 모두 있고 오류 없이 만들어지는지. 폴백(en)이 빠진 메시지를 덮어 화면에서는
// 티가 나지 않으므로 여기서 언어마다 따로 본다.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { FluentBundle, FluentResource, type FluentVariable } from "@fluent/bundle";
import { describe, expect, it } from "vitest";

import { welcomeStrings } from "../welcome/strings";
import { appStrings } from "./app-strings";
import {
  BUNDLED_LOCALES,
  BUNDLED_RESOURCES,
  initialLocales,
  selectedLanguage,
  type Translate,
} from "./localization";

const LOCALES = ["en", "ko"];

function ids(locale: string): string[] {
  return BUNDLED_RESOURCES.filter((r) => r.locale === locale)
    .flatMap((r) => new FluentResource(r.source).body.map((entry) => entry.id))
    .sort();
}

/** 언어 하나만 든 번들(폴백 없이). 없는 id · 만들다 난 오류를 모은다. */
function strictTranslator(locale: string) {
  const bundle = new FluentBundle(locale, { useIsolating: false });
  for (const r of BUNDLED_RESOURCES.filter((r) => r.locale === locale)) {
    expect(bundle.addResource(new FluentResource(r.source))).toEqual([]);
  }
  const used = new Set<string>();
  const problems: string[] = [];
  const t: Translate = (id, vars) => {
    used.add(id);
    const message = bundle.getMessage(id);
    if (!message?.value) {
      problems.push(`${locale}: missing ${id}`);
      return id;
    }
    const errors: Error[] = [];
    const text = bundle.formatPattern(
      message.value,
      vars as Record<string, FluentVariable>,
      errors,
    );
    for (const e of errors) problems.push(`${locale}: ${id}: ${e.message}`);
    return text;
  };
  return { t, used, problems };
}

/** 문자열 객체의 함수(값이 끼는 문자열)를 모두 불러 본다. 인자는 아무 문자열이다 — 선택식은 기본 갈래로 간다. */
function callAll(value: unknown) {
  if (typeof value === "function") (value as (...a: string[]) => unknown)("x", "y");
  else if (value && typeof value === "object") Object.values(value).forEach(callAll);
}

describe("built-in translations", () => {
  it("bundles every built-in language the Rust registry loads", () => {
    expect(BUNDLED_LOCALES).toEqual(LOCALES);
  });

  it("define the same messages in ko and en", () => {
    expect(ids("ko")).toEqual(ids("en"));
    expect(ids("en").length).toBeGreaterThan(0);
  });

  it.each(LOCALES)("have every message the screens use, without format errors (%s)", (locale) => {
    const { t, used, problems } = strictTranslator(locale);
    callAll(welcomeStrings(t));
    callAll(appStrings(t));
    // 플랫폼마다 갈래가 다른 문자열은 갈래를 하나씩 본다.
    for (const platform of ["mac", "win", "linux"] as const) {
      welcomeStrings(t).menu.reveal(platform);
      welcomeStrings(t).actions.reveal(platform);
    }
    expect(problems).toEqual([]);
    expect(used.size).toBeGreaterThan(50);
  });

  it.each(LOCALES)("have every message the native menu and dialogs use (%s)", (locale) => {
    const source = ["menu.rs", "shell_command.rs"]
      .map((file) =>
        readFileSync(
          fileURLToPath(new URL(`../../src-tauri/src/${file}`, import.meta.url)),
          "utf8",
        ),
      )
      .join("\n");
    const nativeIds = [
      ...new Set([...source.matchAll(/"((?:menu|shell-command)-[a-z-]+)"/g)].map((m) => m[1]!)),
    ];
    expect(nativeIds.filter((id) => id.startsWith("shell-command-")).length).toBe(5);
    const { t, problems } = strictTranslator(locale);
    const vars = { app: "Pitwall", target: "/t", previous: "/p", error: "e" };
    for (const id of nativeIds) t(id, vars);
    expect(problems).toEqual([]);
  });
});

describe("initialLocales", () => {
  it("uses the remembered choice, then the browser language, then English", () => {
    expect(initialLocales(["ko", "en"], ["en-US"])).toEqual(["ko", "en"]);
    expect(initialLocales(undefined, ["ko-KR", "en-US"])).toEqual(["ko", "en"]);
    expect(initialLocales(["fr", "en"], ["ja-JP"])).toEqual(["en"]);
    expect(initialLocales(undefined, [])).toEqual(["en"]);
  });
});

describe("selectedLanguage", () => {
  it("shows the language in use when the setting names it differently", () => {
    expect(selectedLanguage("system", ["ko", "en"])).toBe("system");
    expect(selectedLanguage("ko", ["ko", "en"])).toBe("ko");
    expect(selectedLanguage("ko-KR", ["ko", "en"])).toBe("ko");
    expect(selectedLanguage("ja", ["en"])).toBe("en");
  });
});
