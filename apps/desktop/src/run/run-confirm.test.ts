// 첫 실행 확인의 다시 부르기(run-confirm.ts — 17-m6-plan M6-07 ⑥).

import { RpcCallError, type RunConfirmRequired } from "@pitwall/rpc-client";
import { expect, it, vi } from "vitest";

import { RUN_ERROR_CODE, runConfirmOf, withRunConfirm, withRunConfirms } from "./run-confirm";

const ask = (hash: string): RunConfirmRequired => ({
  hash,
  shown: {
    key: "dev",
    name: "dev",
    kind: "shell",
    argv: ["true"],
    cwd: ".",
    profile: null,
    env: [],
    envFiles: [],
    definition: null,
    definitionSource: null,
    secretLooking: false,
    recordable: true,
  },
});
const refused = (hash: string) =>
  new RpcCallError({
    code: RUN_ERROR_CODE.CONFIRM_REQUIRED,
    message: "not seen",
    data: ask(hash) as never,
  });

it("calls again with the hash the person confirmed, and asks again when the content changed", async () => {
  const sent: (string | null)[] = [];
  const answers = [refused("h1"), refused("h2")];
  const call = vi.fn(async (confirmed: string | null) => {
    sent.push(confirmed);
    const next = answers.shift();
    if (next) throw next;
    return "started";
  });
  const asked: string[] = [];
  const result = await withRunConfirm(call, async (data) => {
    asked.push(data.hash);
    return true;
  });
  expect(result).toBe("started");
  expect(sent).toEqual([null, "h1", "h2"]);
  expect(asked).toEqual(["h1", "h2"]);
});

it("does not call again when the person cancels, and throws when nobody can ask", async () => {
  const call = vi.fn(async () => {
    throw refused("h1");
  });
  expect(await withRunConfirm(call, async () => false)).toBeUndefined();
  expect(call).toHaveBeenCalledTimes(1);
  await expect(withRunConfirm(call, undefined)).rejects.toBeInstanceOf(RpcCallError);
});

it("reads only its own error code", () => {
  expect(runConfirmOf(refused("h1"))?.hash).toBe("h1");
  expect(
    runConfirmOf(new RpcCallError({ code: 3001, message: "db", data: ask("h1") as never })),
  ).toBeUndefined();
  expect(runConfirmOf(new Error("x"))).toBeUndefined();
});

/** 사슬의 `sql` 단계가 청한 위험 확인(`3001`, `data.config` 가 그 단계). */
const sqlRefused = (config: string, hash: string) =>
  new RpcCallError({
    code: 3001,
    message: "confirm",
    data: {
      config,
      request: {
        grade: "write",
        statements: [],
        statementsHash: hash,
        effects: [],
        nameRequired: false,
        rejected: null,
      },
      connection: { id: "lite", name: "lite", production: false, readOnly: false },
      files: [],
      transaction: "all",
      sql: "UPDATE t SET v = 1;",
    } as never,
  });

it("collects every confirmation of a chain before it runs, and sends them all again", async () => {
  const sent: unknown[] = [];
  const answers = [refused("h1"), sqlRefused("seed", "s1"), sqlRefused("migrate", "m1")];
  const call = vi.fn(async (confirmed: string | null, sqlConfirmed: unknown) => {
    sent.push([confirmed, sqlConfirmed]);
    const next = answers.shift();
    if (next) throw next;
    return "started";
  });
  const result = await withRunConfirms(
    call,
    async (data) => ({ statementsHash: data.request.statementsHash, grade: "write", name: null }),
    async () => true,
  );
  expect(result).toBe("started");
  expect(sent).toEqual([
    [null, null],
    ["h1", null],
    ["h1", { seed: { statementsHash: "s1", grade: "write", name: null } }],
    [
      "h1",
      {
        seed: { statementsHash: "s1", grade: "write", name: null },
        migrate: { statementsHash: "m1", grade: "write", name: null },
      },
    ],
  ]);
});

it("runs nothing of a chain when the person cancels any confirmation", async () => {
  const call = vi.fn(async () => {
    throw sqlRefused("seed", "s1");
  });
  expect(
    await withRunConfirms(
      call,
      async () => undefined,
      async () => true,
    ),
  ).toBeUndefined();
  expect(call).toHaveBeenCalledTimes(1);
  // `data.config` 가 없는 3001(단독 `sql` 구성의 것)은 이 루프가 답하지 않는다.
  const bare = new RpcCallError({ code: 3001, message: "x", data: { request: {} } as never });
  await expect(
    withRunConfirms(
      async () => {
        throw bare;
      },
      async () => undefined,
      async () => true,
    ),
  ).rejects.toBe(bare);
});
