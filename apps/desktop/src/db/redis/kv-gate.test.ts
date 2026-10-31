// 관문 답 읽기(kv-gate.ts): `3001` 의 Redis 확인만 확인으로 읽기, 거절 명령 · 드라이버 오류 꺼내기, 확인을 받아 다시 부르기.

import { type DbKvConfirmRequired, RpcCallError } from "@pitwall/rpc-client";
import { describe, expect, it, vi } from "vitest";

import { kvBlockedOf, kvConfirmRequiredOf, kvDbErrorOf, withKvConfirm } from "./kv-gate";

const KV: DbKvConfirmRequired = {
  grade: "unboundedWrite",
  action: "delete",
  argv: ["DEL", "a"],
  statementsHash: "h1",
  nameRequired: false,
  rejected: null,
};

const err = (code: number, data: unknown) =>
  new RpcCallError({ code, message: "m", data } as never);

describe("kv-gate", () => {
  it("reads only a Redis confirmation from 3001", () => {
    expect(kvConfirmRequiredOf(err(3001, KV))).toEqual(KV);
    // SQL 의 확인(문장 목록)은 Redis 확인이 아니다.
    expect(kvConfirmRequiredOf(err(3001, { statementsHash: "h", statements: [] }))).toBeUndefined();
    expect(kvConfirmRequiredOf(err(-32001, KV))).toBeUndefined();
  });

  it("reads a blocked command only with its code and reason", () => {
    const data = { reason: "blockedCommand", block: "scanCount", command: "SCAN" };
    expect(kvBlockedOf(err(-32001, data))).toEqual({ block: "scanCount", command: "SCAN" });
    expect(kvBlockedOf(err(-32004, data))).toBeUndefined();
    expect(kvBlockedOf(err(-32001, { ...data, block: "other" }))).toBeUndefined();
    expect(kvBlockedOf(err(-32001, { reason: "readOnlyConnection" }))).toBeUndefined();
  });

  it("reads the driver error of 3003", () => {
    expect(
      kvDbErrorOf(err(3003, { error: { kind: "other", code: "KEY_TOO_LARGE", message: "big" } })),
    ).toEqual({ code: "KEY_TOO_LARGE", message: "big" });
    expect(kvDbErrorOf(err(3001, KV))).toBeUndefined();
  });

  it("asks on 3001 and calls again with the answer, asking again when rejected", async () => {
    const call = vi
      .fn<(c: unknown) => Promise<string>>()
      .mockRejectedValueOnce(err(3001, KV))
      .mockRejectedValueOnce(err(3001, { ...KV, statementsHash: "h2", rejected: "hashMismatch" }))
      .mockResolvedValueOnce("ok");
    const ask = vi.fn(async (r: DbKvConfirmRequired) => ({
      grade: r.grade,
      statementsHash: r.statementsHash,
      name: null,
    }));
    expect(await withKvConfirm(call, ask)).toEqual({ kind: "done", value: "ok" });
    expect(call.mock.calls.map(([c]) => c)).toEqual([
      undefined,
      { grade: "unboundedWrite", statementsHash: "h1", name: null },
      { grade: "unboundedWrite", statementsHash: "h2", name: null },
    ]);
    expect(ask.mock.calls[1]![0].rejected).toBe("hashMismatch");
  });

  it("ends cancelled or busy without calling again, and passes other errors through", async () => {
    const call = vi.fn(async () => Promise.reject(err(3001, KV)));
    expect(await withKvConfirm(call, async () => undefined)).toEqual({ kind: "cancelled" });
    expect(await withKvConfirm(call, async () => "busy" as const)).toEqual({ kind: "busy" });
    expect(call).toHaveBeenCalledTimes(2);
    const refused = err(-32001, { reason: "readOnlyConnection" });
    await expect(
      withKvConfirm(
        () => Promise.reject(refused),
        async () => undefined,
      ),
    ).rejects.toBe(refused);
  });
});
