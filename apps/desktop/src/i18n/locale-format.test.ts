// 날짜 · 숫자를 고른 UI 언어로 포맷하는가(07-extensibility.md §4.1): 화면 코드에서 로캘 없이(`toLocaleString()`,
// `toLocaleTimeString(undefined, …)`, `new Intl.DateTimeFormat(undefined, …)`) 포맷하면 시스템 로캘을 따라 영어 UI 에 「오전 07:20」 이
// 섞인다(M5-13b). 로캘은 `useI18n().locales[0]` 을 넘긴다.
//
// 보지 않는 것: 로캘을 변수로 넘겼는데 그 값이 `undefined` 인 경우, 글자 붙이기로 만든 호출, 정렬(`Intl.Collator` ·
// `localeCompare`) — 정렬은 포맷이 아니라 여기서 걸지 않는다.

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, it } from "vitest";

const SRC = join(__dirname, "..");
/** 로캘 없이 포맷하는 호출: 인자가 비었거나 첫 인자가 `undefined`. */
const NO_LOCALE =
  /\.toLocale(?:Date|Time)?String\(\s*(?:\)|undefined\b)|new Intl\.(?:DateTimeFormat|NumberFormat|RelativeTimeFormat|ListFormat|PluralRules)\(\s*(?:\)|undefined\b)/;

it("formats dates and numbers in the UI language, not the system locale", () => {
  const files = readdirSync(SRC, { recursive: true, encoding: "utf8" }).filter(
    (f) => /\.tsx?$/.test(f) && !/\.test\.tsx?$/.test(f),
  );
  expect(files.length).toBeGreaterThan(100);
  const found: string[] = [];
  for (const file of files) {
    const lines = readFileSync(join(SRC, file), "utf8").split("\n");
    lines.forEach((line, i) => {
      if (NO_LOCALE.test(line)) found.push(`${file}:${i + 1}: ${line.trim()}`);
    });
  }
  expect(found).toEqual([]);
});
