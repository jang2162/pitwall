// 사슬 이벤트의 알림 글(run-chain.tsx — 17-m6-plan M6-21).

import type { RunChainEvent } from "@pitwall/rpc-client";
import { expect, it } from "vitest";

import { chainNotice } from "./run-chain";

const T = {
  failed: (config: string, step: string, message: string) => `failed ${config} ${step} ${message}`,
  stopped: (config: string, step: string) => `stopped ${config} ${step}`,
};

const event = (state: RunChainEvent["state"], step: string | null): RunChainEvent => ({
  chainId: "c1",
  projectId: "p",
  config: "app",
  state,
  runs: [],
  step,
  message: step ? "exited with code 3" : null,
  launchedBy: { kind: "window", window: null },
});

it("tells which step stopped a chain, and nothing while it runs or after it finished", () => {
  expect(chainNotice(event("failed", "seed"), T)).toEqual({
    level: "error",
    title: "failed app seed exited with code 3",
  });
  expect(chainNotice(event("stopped", "slow"), T)).toEqual({
    level: "info",
    title: "stopped app slow",
  });
  expect(chainNotice(event("running", null), T)).toBeUndefined();
  expect(chainNotice(event("done", null), T)).toBeUndefined();
});
