// 실행 구성 목록(run-configs.ts)이 붙는 곳을 바꿀 때(18-m7-plan M7-14 리뷰 권고 2 — M7-11b): 다른 머신으로 붙으면 앞 머신의 목록을
// 곧바로 비우고, 앞 머신의 늦은 답은 버린다. 같은 곳으로 다시 붙으면(StrictMode 의 떼었다 붙이기) 목록을 그대로 둔다.

import type { RunConfigList } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import { RunConfigs, type RunConfigsClient } from "./run-configs";

function list(id: string): RunConfigList {
  return {
    configs: [{ id, config: { type: "npm", name: id } } as never],
    unusable: [],
    detected: [],
    problem: null,
  };
}

/** `run.config.list` 의 답을 손으로 정하는 연결. */
function machine() {
  const pending: ((value: RunConfigList) => void)[] = [];
  const client = {
    call: (method: string) => {
      if (method !== "run.config.list") return Promise.reject(new Error(method));
      return new Promise<RunConfigList>((resolve) => pending.push(resolve));
    },
    subscribe: async () => ({ unsubscribe: async () => {} }),
  } as unknown as RunConfigsClient;
  return { client, answer: (value: RunConfigList) => pending.shift()!(value), pending };
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("RunConfigs.connect", () => {
  it("drops the old machine's list as soon as it moves to another one, and ignores its late answer", async () => {
    const configs = new RunConfigs();
    const here = machine();
    const mini = machine();
    const off = configs.connect(here.client, "p1", null);
    here.answer(list("local-build"));
    await flush();
    expect(configs.groups().configs.map((c) => c.id)).toEqual(["local-build"]);

    // 앞 머신의 두 번째 읽기가 걸린 채로 다른 머신에 붙는다.
    void configs.reload();
    off();
    configs.connect(mini.client, "r1", null);
    expect(configs.store.getState().list).toBeUndefined();
    expect(configs.current()).toBeUndefined();
    here.answer(list("late-local"));
    await flush();
    expect(configs.store.getState().list).toBeUndefined();
    mini.answer(list("mini-build"));
    await flush();
    expect(configs.groups().configs.map((c) => c.id)).toEqual(["mini-build"]);
  });

  it("keeps the list when it reconnects to the same place", async () => {
    const configs = new RunConfigs();
    const here = machine();
    const off = configs.connect(here.client, "p1", null);
    here.answer(list("local-build"));
    await flush();
    off();
    configs.connect(here.client, "p1", null);
    expect(configs.groups().configs.map((c) => c.id)).toEqual(["local-build"]);
  });
});
