/// <reference types="node" />
// 에디터의 기준값이 Rust 정본(crates/pw-fs/src/limits.rs)과 같은지 본다 — 한쪽만 고치면 판정과 에디터가 다른 기준으로 열린다.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { expect, it } from "vitest";

import * as limits from "./limits";

const RUST = join(
  dirname(fileURLToPath(import.meta.url)),
  "../../../../crates/pw-fs/src/limits.rs",
);

/** `const NAME: 형 = 식;` 의 값. 식은 정수와 앞서 정의한 이름의 곱이다(`5 * MIB`). */
function rustConstants(source: string): Map<string, number> {
  const values = new Map<string, number>();
  for (const [, name = "", expr = ""] of source.matchAll(/const (\w+): \w+ = ([^;]+);/g)) {
    const factors = expr.split("*").map((f) => f.trim().replaceAll("_", ""));
    values.set(
      name,
      factors.reduce((product, f) => {
        const value = /^\d+$/.test(f) ? Number(f) : values.get(f);
        if (value === undefined) throw new Error(`unknown factor ${f} in ${name}`);
        return product * value;
      }, 1),
    );
  }
  return values;
}

it("matches every threshold in crates/pw-fs/src/limits.rs", () => {
  const rust = rustConstants(readFileSync(RUST, "utf8"));
  const exported = Object.entries(limits).filter(([, v]) => typeof v === "number");
  const shared = ["SNIFF_BYTES", "EDITABLE_MAX", "READ_ONLY_MAX", "LONG_LINE_CHARS"];
  for (const name of [...shared, "HEAD_PREVIEW_BYTES", "HEX_PAGE_BYTES"])
    expect(limits[name as keyof typeof limits], name).toBe(rust.get(name));
  // 이쪽에만 있는 수 상수가 없다.
  for (const [name] of exported) expect(rust.has(name), name).toBe(true);
  // 결정 M1-D9 · M1-D10 의 값 그대로.
  expect([limits.EDITABLE_MAX, limits.READ_ONLY_MAX, limits.HEX_PAGE_BYTES]).toEqual([
    5 * 1024 * 1024,
    50 * 1024 * 1024,
    64 * 1024,
  ]);
});
