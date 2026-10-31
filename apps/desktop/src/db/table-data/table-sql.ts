// 테이블 데이터 문서가 만드는 SQL 글(14-m3-plan.md M3-17): `SELECT * FROM <테이블> WHERE … ORDER BY …`, 헤더 클릭 정렬의 ORDER BY,
// FK 따라가기의 WHERE.
//
// - 테이블 이름은 메타데이터의 정본 이름을 방언대로 늘 따옴표 친다(탐색기 M3-13 의 sql-names `qualifiedName(…, true)` — PG 는
//   스키마, MySQL 은 DB 를 붙인다). 열 이름(ORDER BY · FK 의 WHERE)은 사람이 칸에서 보고 고치는 글이라 필요할 때만 따옴표를 친다
//   (`quoteName` — 콘솔로 끌어 넣는 이름과 같은 모양).
// - **화면은 조각을 검사하지 않는다**: 만든 글은 `db.query.execute { queryOnly: true }` 로 가고 Host 의 관문이 분류해 조회 문장
//   하나가 아니면 거절한다(`notAQuery` — Host 가 정본, M3-05 · M3-17). 조각은 줄을 나눠 붙인다 — WHERE 칸의 `--` 주석이 뒤의
//   ORDER BY 를 먹지 않게.
// - FK 의 값 리터럴은 `db.sql.insertText`(M3-16 이 만든 리터럴 정본 `pw_db_sql::literal` — 서버 설정과 무관한 꼴)에서 받는다.

import type { DbValue, ObjectRef, RpcClient, SqlDialect } from "@pitwall/rpc-client";

import { qualifiedName, quoteName } from "../explorer/sql-names";

/** 위 막대의 조각(빈 글이면 그 절을 두지 않는다). */
export interface TableQuery {
  where: string;
  orderBy: string;
}

/** 테이블을 읽는 글. */
export function tableSelect(dialect: SqlDialect, object: ObjectRef, query: TableQuery): string {
  const lines = [`SELECT * FROM ${qualifiedName(dialect, object, true)}`];
  if (query.where.trim() !== "") lines.push(`WHERE ${query.where.trim()}`);
  if (query.orderBy.trim() !== "") lines.push(`ORDER BY ${query.orderBy.trim()}`);
  return lines.join("\n");
}

/** 헤더 클릭 정렬의 ORDER BY 조각. 정렬을 풀면 빈 글. */
export function orderByColumn(
  dialect: SqlDialect,
  column: string,
  direction: "asc" | "desc" | undefined,
): string {
  if (!direction) return "";
  return `${quoteName(dialect, column)} ${direction === "asc" ? "ASC" : "DESC"}`;
}

/** 값 하나의 SQL 리터럴(`db.sql.insertText` 가 만든 INSERT 글에서 꺼낸다). */
export async function sqlLiteral(
  client: Pick<RpcClient, "call">,
  dialect: SqlDialect,
  value: DbValue,
): Promise<string> {
  const { text } = await client.call("db.sql.insertText", {
    dialect,
    table: null,
    columns: ["v"],
    rows: [[value]],
  });
  // `INSERT INTO MY_TABLE (<v>) VALUES (<리터럴>);\n` — 열 이름 · 자리표시 테이블에는 `) VALUES (` 가 없다.
  const head = text.indexOf(") VALUES (");
  const tail = text.lastIndexOf(");");
  if (head < 0 || tail <= head) throw new Error(`unexpected INSERT text: ${text}`);
  return text.slice(head + ") VALUES (".length, tail);
}

/** FK 따라가기의 WHERE: 참조 열마다 `<열> = <리터럴>` 을 AND 로. 값이 NULL 인 열이 있으면 `undefined`(가리키는 행이 없다). */
export async function foreignKeyWhere(
  client: Pick<RpcClient, "call">,
  dialect: SqlDialect,
  pairs: { column: string; value: DbValue }[],
): Promise<string | undefined> {
  if (pairs.length === 0 || pairs.some((p) => p.value === "null")) return undefined;
  const parts: string[] = [];
  for (const p of pairs)
    parts.push(`${quoteName(dialect, p.column)} = ${await sqlLiteral(client, dialect, p.value)}`);
  return parts.join(" AND ");
}
