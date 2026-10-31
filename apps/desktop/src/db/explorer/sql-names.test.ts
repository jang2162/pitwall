// 탐색기가 만드는 SQL 글의 이름(sql-names.ts): 드라이버의 따옴표, 따옴표가 필요 없는 이름은 그대로, 예약어 · 대문자(PG) · 공백 ·
// 안의 따옴표, `SELECT * FROM` · 「행 수」 의 스키마(PG) · DB(MySQL) 붙이기.

import { describe, expect, it } from "vitest";

import { countSql, dropText, qualifiedName, quoteName } from "./sql-names";

describe("quoteName", () => {
  it.each([
    ["postgres", "invoice", "invoice"],
    ["postgres", "Invoice", '"Invoice"'],
    ["postgres", "order", '"order"'],
    ["postgres", "line items", '"line items"'],
    ["postgres", 'a"b', '"a""b"'],
    ["postgres", "1st", '"1st"'],
    ["mysql", "Invoice", "Invoice"],
    ["mysql", "user", "`user`"],
    ["mysql", "a`b", "`a``b`"],
    ["sqlite", "Invoice", "Invoice"],
    ["sqlite", "select", '"select"'],
    ["postgres", "current_user", '"current_user"'],
    ["postgres", "localtimestamp", '"localtimestamp"'],
    ["postgres", "session_user", '"session_user"'],
  ] as const)("%s %s → %s", (driver, name, quoted) => {
    expect(quoteName(driver, name)).toBe(quoted);
  });
});

describe("qualifiedName", () => {
  it("adds a PostgreSQL schema other than public and a MySQL database", () => {
    expect(qualifiedName("postgres", { database: "billing", schema: "public", name: "t" })).toBe(
      "t",
    );
    expect(qualifiedName("postgres", { database: "billing", schema: "Sales", name: "t" })).toBe(
      '"Sales".t',
    );
    expect(qualifiedName("mysql", { database: "shop", schema: null, name: "t" })).toBe("shop.t");
    expect(qualifiedName("sqlite", { database: null, schema: null, name: "t" })).toBe("t");
  });
});

it("drops a bare name or a qualified SELECT, and counts by the qualified name", () => {
  const object = { database: "shop", schema: null, name: "order" };
  expect(dropText("mysql", object, false)).toBe("`order`");
  expect(dropText("mysql", object, true)).toBe("SELECT * FROM shop.`order`");
  expect(countSql("mysql", object)).toBe("SELECT count(*) FROM `shop`.`order`");
});

it("always quotes the names it counts by — no one reads that SQL", () => {
  expect(countSql("postgres", { database: "b", schema: "public", name: "current_user" })).toBe(
    'SELECT count(*) FROM "current_user"',
  );
  expect(countSql("postgres", { database: "b", schema: "sales", name: "t" })).toBe(
    'SELECT count(*) FROM "sales"."t"',
  );
  expect(countSql("sqlite", { database: null, schema: null, name: "t" })).toBe(
    'SELECT count(*) FROM "t"',
  );
});
