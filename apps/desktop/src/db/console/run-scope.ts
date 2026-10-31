// SQL 콘솔의 실행 범위(14-m3-plan.md M3-15): 선택이 있으면 선택과 겹치는 문장들, 없으면 커서의 「현재 문장」(M3-04 의
// `db.sql.statementAt` — 빈 줄이 아니라 구분자 기준, 두 문장 사이면 앞 문장). 나누기 · 현재 문장의 규칙은 Host 하나가 정한다 —
// 여기서는 그 결과의 번호만 고른다.
//
// 보내는 SQL 은 `state.doc.toString()` 이다. CodeMirror 는 줄바꿈(`\r\n` · `\r` · `\n`)을 한 단위로 세고 `toString()` 은 `\n` 으로
// 잇는다 — 커서 오프셋은 그 글의 UTF-16 오프셋이다. 파일 원문(CRLF)을 보내면 Host 가 매긴 범위 · 오프셋이 줄마다 한 칸씩 밀린다.

import type { SqlDialect, SqlRange, UnaryMethods } from "@pitwall/rpc-client";

type Call = <K extends "db.sql.split" | "db.sql.statementAt">(
  method: K,
  params: UnaryMethods[K]["params"],
) => Promise<UnaryMethods[K]["result"]>;

/** 선택(UTF-16 오프셋, `from <= to`). */
export interface Selection {
  from: number;
  to: number;
}

/** 선택과 겹치는 문장의 번호. 선택이 문장 사이의 공백 · 주석에만 걸치면 빈 목록이다. */
export function overlapping(ranges: readonly SqlRange[], selection: Selection): number[] {
  return ranges.flatMap((range, index) =>
    range.from < selection.to && selection.from < range.to ? [index] : [],
  );
}

/** 돌릴 문장 번호(`db.query.execute` 의 `statements`). 돌릴 문장이 없으면 빈 목록. */
export async function statementsToRun(
  call: Call,
  dialect: SqlDialect,
  sql: string,
  selection: Selection,
): Promise<number[]> {
  if (selection.from === selection.to) {
    const { statement } = await call("db.sql.statementAt", {
      dialect,
      sql,
      offset: selection.from,
    });
    return statement ? [statement.index] : [];
  }
  const { statements } = await call("db.sql.split", { dialect, sql });
  return overlapping(statements, selection);
}
