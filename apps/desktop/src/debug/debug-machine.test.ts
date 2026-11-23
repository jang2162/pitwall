// 디버그 연결의 머신(18-m7-plan M7-21 ① — 메인 판정: 디버그는 활성 체크아웃 머신에서). 거터의 문서가 디버그 머신의 문서일 때만 그
// 머신의 브레이크포인트를 둔다 — 두 머신에 같은 경로가 있어도 섞이지 않는다.

import { describe, expect, it } from "vitest";

import { checkoutKey, LOCAL_MACHINE, qualify } from "../checkout/checkout";
import { debugDocument } from "./debug-feature";

const MINI = "SHA256:mini/fingerprint";
const PATH = "/work/demo/src/app.py";

describe("debugDocument", () => {
  it("takes a document of the debug machine as that machine's path and worktree", () => {
    expect(debugDocument(MINI, qualify(MINI, PATH), checkoutKey({ machine: MINI }))).toEqual({
      path: PATH,
      worktree: undefined,
    });
    expect(
      debugDocument(MINI, qualify(MINI, PATH), checkoutKey({ machine: MINI, worktree: "/wt" })),
    ).toEqual({ path: PATH, worktree: "/wt" });
    expect(debugDocument(LOCAL_MACHINE, PATH, "/wt")).toEqual({ path: PATH, worktree: "/wt" });
  });

  it("leaves out a document of another machine, even with the same path", () => {
    expect(debugDocument(MINI, PATH, undefined)).toBeUndefined();
    expect(debugDocument(LOCAL_MACHINE, qualify(MINI, PATH), checkoutKey({ machine: MINI }))).toBe(
      undefined,
    );
    expect(debugDocument(MINI, qualify("SHA256:box/x", PATH), undefined)).toBeUndefined();
  });
});
