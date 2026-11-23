// 디버그 연결의 머신(18-m7-plan M7-21 ① — 메인 판정: 디버그는 활성 체크아웃 머신에서). 거터의 문서가 디버그 머신의 문서일 때만 그
// 머신의 브레이크포인트를 둔다 — 두 머신에 같은 경로가 있어도 섞이지 않는다.

import { describe, expect, it, vi } from "vitest";

import { checkoutKey, LOCAL_MACHINE, qualify } from "../checkout/checkout";
import { activeWorktreeStore } from "../open-with/active-worktree";
import { debugDocument, openSavedBreakpoint } from "./debug-feature";

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

// M7-21 리뷰 F2: 목록에서 연 저장된 브레이크포인트(접두 없는 그 머신의 루트 + 상대 경로)는 디버그 연결의 머신을 실어 연다 — 루트를
// 묻는 사이 활성 체크아웃이 이 머신으로 바뀌어도 이 머신의 같은 경로로 붙지 않는다.
describe("openSavedBreakpoint", () => {
  it("opens that machine's file even when the active checkout moved while asking for the root", async () => {
    let answer!: (v: unknown) => void;
    const client = {
      call: vi.fn(() => new Promise((r) => (answer = r))),
    } as never;
    const open = vi.fn(async () => undefined);
    const opening = openSavedBreakpoint(client, "r1", MINI, { open } as never, "src/lib.py", 7);
    activeWorktreeStore.setState({ top: undefined });
    answer({ roots: ["/work/demo"] });
    await opening;
    expect(open).toHaveBeenCalledWith({
      path: "/work/demo/src/lib.py",
      machine: MINI,
      line: 7,
      column: 1,
    });
  });

  it("opens nothing when the root cannot be read", async () => {
    const open = vi.fn();
    const client = { call: vi.fn(async () => Promise.reject(new Error("closed"))) } as never;
    vi.spyOn(console, "warn").mockImplementation(() => {});
    await openSavedBreakpoint(client, "r1", MINI, { open } as never, "src/lib.py", 7);
    expect(open).not.toHaveBeenCalled();
  });
});
