// 탐색기가 만드는 SQL 글의 이름(14-m3-plan.md M3-13): 콘솔로 끌어 넣는 이름 · `SELECT * FROM …`, 「행 수」 의 `count(*)`.
//
// - 따옴표는 드라이버의 것이다: PG · SQLite 는 `"…"`, MySQL/MariaDB 는 `` `…` ``. 안의 같은 따옴표는 두 번 쓴다.
// - 따옴표 없이 읽히는 이름(글자 · 숫자 · `_`, 숫자로 시작하지 않음, 예약어 아님 — PG 는 소문자만)은 그대로 둔다. 사람이 콘솔에
//   치는 모양과 같게 하려는 것이다. 예약어 목록은 흔한 것만이다 — 빠진 예약어는 따옴표 없이 나가 콘솔에서 문법 오류가 된다(데이터를
//   건드리지 않는다).
// - 끌어 넣는 이름은 이름 하나, `SELECT * FROM` · 「행 수」 는 PG 는 스키마(`public` 이 아니면), MySQL 은 DB 를 붙인다 — 콘솔이 다른
//   DB · 스키마를 보고 있어도 그 테이블을 가리킨다.

import type { DriverKind, ObjectRef } from "@pitwall/rpc-client";

const RESERVED = new Set(
  (
    "all and any as asc between by case check column constraint create cross default delete desc " +
    "distinct drop else end exists false fetch for foreign from full grant group having in index " +
    "inner insert intersect into is join key left like limit not null offset on or order outer " +
    "primary references right select set table then to true union unique update user using values " +
    "when where with"
  ).split(" "),
);

function plain(driver: DriverKind, name: string): boolean {
  const pattern = driver === "postgres" ? /^[a-z_][a-z0-9_]*$/ : /^[A-Za-z_][A-Za-z0-9_]*$/;
  return pattern.test(name) && !RESERVED.has(name.toLowerCase());
}

/** 이름 하나(필요할 때만 따옴표). */
export function quoteName(driver: DriverKind, name: string): string {
  if (plain(driver, name)) return name;
  if (driver === "mysql") return `\`${name.replaceAll("`", "``")}\``;
  return `"${name.replaceAll('"', '""')}"`;
}

/** 다른 DB · 스키마를 보는 콘솔에서도 그 객체를 가리키는 이름. */
export function qualifiedName(driver: DriverKind, object: ObjectRef): string {
  const name = quoteName(driver, object.name);
  if (driver === "postgres" && object.schema && object.schema !== "public")
    return `${quoteName(driver, object.schema)}.${name}`;
  if (driver === "mysql" && object.database) return `${quoteName(driver, object.database)}.${name}`;
  return name;
}

/** 콘솔로 끌어 넣는 글: 이름, `⌥` 면 `SELECT * FROM <이름>`. */
export function dropText(driver: DriverKind, object: ObjectRef, select: boolean): string {
  return select ? `SELECT * FROM ${qualifiedName(driver, object)}` : quoteName(driver, object.name);
}

/** 「행 수」 의 조회. */
export function countSql(driver: DriverKind, object: ObjectRef): string {
  return `SELECT count(*) FROM ${qualifiedName(driver, object)}`;
}
