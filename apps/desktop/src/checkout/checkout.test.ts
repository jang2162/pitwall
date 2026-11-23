import { expect, it } from "vitest";

import {
  checkoutKey,
  checkoutOfKey,
  LOCAL_MACHINE,
  machineOf,
  qualify,
  unqualify,
} from "./checkout";

const MINI = "SHA256:ab/c+d=";

it("keeps this machine's paths and keys as they were", () => {
  expect(qualify(LOCAL_MACHINE, "/work/app/a.ts")).toBe("/work/app/a.ts");
  expect(checkoutKey({ machine: LOCAL_MACHINE })).toBeUndefined();
  expect(checkoutKey({ machine: LOCAL_MACHINE, worktree: "/work/wt" })).toBe("/work/wt");
  expect(checkoutOfKey(undefined)).toEqual({ machine: LOCAL_MACHINE });
  expect(checkoutOfKey("/work/wt")).toEqual({ machine: LOCAL_MACHINE, worktree: "/work/wt" });
});

it("prefixes another machine's paths so the same path on two machines stays apart", () => {
  const remote = qualify(MINI, "/work/app/a.ts");
  expect(remote).not.toBe("/work/app/a.ts");
  expect(unqualify(remote)).toEqual({ machine: MINI, path: "/work/app/a.ts" });
  expect(machineOf(remote)).toBe(MINI);
  for (const checkout of [{ machine: MINI }, { machine: MINI, worktree: "/work/app-wt/ci" }]) {
    expect(checkoutOfKey(checkoutKey(checkout))).toEqual(checkout);
  }
  expect(machineOf("/work/app/a.ts", checkoutKey({ machine: MINI }))).toBe(MINI);
  expect(machineOf("/work/app/a.ts")).toBe(LOCAL_MACHINE);
});
