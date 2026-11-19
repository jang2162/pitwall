// 첫 실행 확인의 다시 부르기(run-confirm.ts — 17-m6-plan M6-07 ⑥).

import { RpcCallError, type RunConfirmRequired } from "@pitwall/rpc-client";
import { expect, it, vi } from "vitest";

import { RUN_ERROR_CODE, runConfirmOf, withRunConfirm } from "./run-confirm";

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
