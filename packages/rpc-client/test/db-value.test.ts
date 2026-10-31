// DB 셀 값(`DbValue`, 14-m3-plan.md 결정 M3-D8)이 생성물에 있고 Rust 의 와이어 형식과 맞는지. 메서드가 아직 참조하지 않아
// `rpc_type!` 로 내보낸다(CONTRIBUTING.md 「RPC 타입 생성」). 아래 `@ts-expect-error` 줄은 `tsc --noEmit` 에서 에러여야 한다.
import { describe, expect, it } from "vitest";

import type { Capabilities, DbError, DbValue, RowBatch } from "../src";

// crates/pw-db-types/src/value/tests.rs 의 스냅숏과 같은 값들.
const wire: DbValue[] = [
  { int: "9007199254740992" },
  { int: 9007199254740991 },
  { int: "18446744073709551615" },
  { int: 5 },
  { decimal: "0.1" },
  { bytes: { hex: "0xdeadbeef", size: 5, truncated: true } },
  "null",
  { float: "NaN" },
  { truncated: { of: "text", prefix: "가", size: 9, typeName: null } },
  { array: [{ int: 1 }, "null"] },
  { other: { text: "'a':1", typeName: "tsvector" } },
];

function typeChecks(): void {
  // @ts-expect-error 변형 이름이 아니다
  const unknownKind: DbValue = { bigint: "1" };
  // @ts-expect-error 바이트는 hex · 크기 · truncated 를 함께 싣는다
  const bareBytes: DbValue = { bytes: "0xdead" };
  // @ts-expect-error 값 하나에 변형은 하나다
  const twoKinds: DbValue = { int: 1, text: "1" };
  // @ts-expect-error null 은 JSON null 이 아니라 "null" 이다
  const jsonNull: DbValue = null;
  void [unknownKind, bareBytes, twoKinds, jsonNull];
}

describe("DbValue", () => {
  it("is generated with the wire shapes the Rust side writes", () => {
    void typeChecks;
    const batch: RowBatch = { rows: [wire], end: { rowsAffected: 1 } };
    expect(JSON.parse(JSON.stringify(batch))).toEqual(batch);
    // 2^53 이상은 문자열이라 JS 로 읽어도 자릿수를 잃지 않는다.
    expect(BigInt((wire[0] as { int: string }).int)).toBe(2n ** 53n);
  });

  it("comes with the driver capability and error types", () => {
    const caps: Capabilities = {
      sql: true,
      kv: false,
      serverCursor: true,
      cancel: true,
      transactions: true,
      savepoints: true,
      readOnlyTransaction: true,
      explain: true,
      metadata: true,
      schemas: true,
      multipleDatabases: true,
      sslModes: ["disable", "verify-full"],
    };
    const error: DbError = { kind: "readOnly", message: "cannot execute INSERT", code: "25006" };
    expect([caps.sslModes, error.kind]).toEqual([["disable", "verify-full"], "readOnly"]);
  });
});
