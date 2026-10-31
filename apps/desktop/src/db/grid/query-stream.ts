// `db.query.execute` 스트림(M3-05)을 문장마다의 결과 버퍼로 옮긴다(M3-16). SQL 콘솔(M3-15)이 결과 탭마다 하나씩 쓴다.
//
// 항목 차례는 `accepted` → 문장마다 `started` → `columns` → `rows` … → `done` | `error` 다. 행을 돌려주지 않는 문장은 `columns`
// 없이 `done` 이다(버퍼의 열이 비고 상태 줄이 바뀐 행 수를 보인다).

import type {
  DbQueryEvent,
  DbQueryExecuteParams,
  RpcClient,
  RiskGrade,
  SqlRange,
} from "@pitwall/rpc-client";

import { ResultBuffer } from "./result-buffer";

export interface StatementResult {
  index: number;
  range: SqlRange;
  grade: RiskGrade;
  buffer: ResultBuffer;
}

export class QueryRun {
  executionId: string | undefined;
  statements = 0;
  readonly results: StatementResult[] = [];
  private readonly listeners = new Set<() => void>();

  constructor(private readonly makeBuffer: () => ResultBuffer = () => new ResultBuffer()) {}

  result(index: number): StatementResult | undefined {
    return this.results.find((r) => r.index === index);
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  apply(event: DbQueryEvent) {
    switch (event.kind) {
      case "accepted":
        this.executionId = event.executionId;
        this.statements = event.statements;
        break;
      case "started":
        this.results.push({
          index: event.index,
          range: event.range,
          grade: event.grade,
          buffer: this.makeBuffer(),
        });
        break;
      case "columns":
        this.result(event.index)?.buffer.setColumns(event.columns);
        return;
      case "rows":
        this.result(event.index)?.buffer.append(event.rows);
        return;
      case "done":
        this.result(event.index)?.buffer.finish({
          kind: "done",
          truncated: event.truncated,
          elapsedMs: event.elapsedMs,
          rowsAffected: event.rowsAffected,
        });
        break;
      case "error":
        this.result(event.index)?.buffer.finish({
          kind: "error",
          message: event.error.message,
          cancelled: event.error.kind === "cancelled",
          elapsedMs: event.elapsedMs,
        });
        break;
    }
    for (const l of this.listeners) l();
  }

  /** 스트림을 끝까지 읽는다. `signal` 로 그만 읽으면 Host 의 스트림도 취소된다(실행 취소는 `db.query.cancel`). */
  async run(client: RpcClient, params: DbQueryExecuteParams, signal?: AbortSignal) {
    for await (const event of client.stream("db.query.execute", params, { signal })) {
      this.apply(event);
    }
  }
}
