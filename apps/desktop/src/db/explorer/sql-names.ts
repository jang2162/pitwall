// 탐색기가 만드는 SQL 글의 이름(14-m3-plan.md M3-13): 콘솔로 끌어 넣는 이름 · `SELECT * FROM …`, 「행 수」 의 `count(*)`.
//
// - 따옴표는 드라이버의 것이다: PG · SQLite 는 `"…"`, MySQL/MariaDB 는 `` `…` ``. 안의 같은 따옴표는 두 번 쓴다.
// - 콘솔로 끌어 넣는 이름은 따옴표 없이 읽히는 이름(글자 · 숫자 · `_`, 숫자로 시작하지 않음, 예약어 아님 — PG 는 소문자만)이면
//   그대로 둔다. 사람이 콘솔에 치는 모양과 같게 하려는 것이다. 예약어 목록은 흔한 것과 SQL 값 함수 이름(`current_user` 등 — PG 는
//   FROM 에서 함수로 읽어 문법 오류 없이 다른 질의가 된다)이다. 그래도 빠진 것이 있으면 따옴표 없이 나간다 — 사람이 콘솔에서 보고
//   고친다.
// - 「행 수」 의 이름은 늘 따옴표를 치고 PG 는 스키마를 `public` 이어도 붙인다 — 사람이 보지 않는 글이라 모양보다 정확함이
//   먼저다(`search_path` 의 앞자리 — 기본 `"$user"` — 에 같은 이름의 테이블이 있으면 그것을 세지 않게).
// - 끌어 넣는 이름은 이름 하나, `SELECT * FROM` · 「행 수」 는 PG 는 스키마(`public` 이 아니면), MySQL 은 DB 를 붙인다 — 콘솔이 다른
//   DB · 스키마를 보고 있어도 그 테이블을 가리킨다.

import type { DriverKind, ObjectRef } from "@pitwall/rpc-client";

const RESERVED = new Set(
  (
    "all and any as asc between by case check column constraint create cross default delete desc " +
    "distinct drop else end exists false fetch for foreign from full grant group having in index " +
    "inner insert intersect into is join key left like limit not null offset on or order outer " +
    "primary references right select set table then to true union unique update user using values " +
    "when where with " +
    // SQL 값 함수 — 따옴표 없이 FROM 에 두면 PG 는 테이블이 아니라 함수 결과(한 행)를 읽는다.
    "current_catalog current_date current_role current_schema current_time current_timestamp " +
    "current_user localtime localtimestamp session_user"
  ).split(" "),
);

function plain(driver: DriverKind, name: string): boolean {
  const pattern = driver === "postgres" ? /^[a-z_][a-z0-9_]*$/ : /^[A-Za-z_][A-Za-z0-9_]*$/;
  return pattern.test(name) && !RESERVED.has(name.toLowerCase());
}

function quoted(driver: DriverKind, name: string): string {
  if (driver === "mysql") return `\`${name.replaceAll("`", "``")}\``;
  return `"${name.replaceAll('"', '""')}"`;
}

/** 이름 하나(필요할 때만 따옴표). */
export function quoteName(driver: DriverKind, name: string): string {
  return plain(driver, name) ? name : quoted(driver, name);
}

/**
 * 다른 DB · 스키마를 보는 콘솔에서도 그 객체를 가리키는 이름. `always` 면 모든 이름에 따옴표를 치고 PG 의 `public` 스키마도
 * 붙인다.
 */
export function qualifiedName(driver: DriverKind, object: ObjectRef, always = false): string {
  const q = always ? quoted : quoteName;
  const name = q(driver, object.name);
  if (driver === "postgres" && object.schema && (always || object.schema !== "public"))
    return `${q(driver, object.schema)}.${name}`;
  if (driver === "mysql" && object.database) return `${q(driver, object.database)}.${name}`;
  return name;
}

/** 콘솔로 끌어 넣는 글: 이름, `⌥` 면 `SELECT * FROM <이름>`. */
export function dropText(driver: DriverKind, object: ObjectRef, select: boolean): string {
  return select ? `SELECT * FROM ${qualifiedName(driver, object)}` : quoteName(driver, object.name);
}

/** 「행 수」 의 조회(이름은 늘 따옴표). */
export function countSql(driver: DriverKind, object: ObjectRef): string {
  return `SELECT count(*) FROM ${qualifiedName(driver, object, true)}`;
}
