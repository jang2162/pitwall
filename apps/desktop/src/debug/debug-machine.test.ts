// 디버그 연결의 머신(18-m7-plan M7-21 ① — 메인 판정: 디버그는 활성 체크아웃 머신에서). 거터의 문서가 디버그 머신의 문서일 때만 그
// 머신의 브레이크포인트를 둔다 — 두 머신에 같은 경로가 있어도 섞이지 않는다.

import { describe, expect, it, vi } from "vitest";

import { checkoutKey, LOCAL_MACHINE, qualify } from "../checkout/checkout";
import { activeWorktreeStore } from "../open-with/active-worktree";
import { debugDocument, openSavedBreakpoint } from "./debug-feature";
import { DebugSessions } from "./debug-session";

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

// M7-21 리뷰 B1: 세션은 시작한 머신을 기억한다 — 창의 디버그 연결 머신(`setDeps`)이 그 사이 바뀌어도 그 세션의 프레임 · 멈춘 줄은 그
// 세션의 머신으로 연다(지금 연결의 머신이 아니다).
describe("a debug session's machine", () => {
  it("opens its stopped frame on the machine it started on", async () => {
    let listener: ((e: unknown) => void) | undefined;
    const client = {
      subscribe: vi.fn(async (_p: unknown, l: (e: unknown) => void) => {
        listener = l;
        return { unsubscribe: async () => {} };
      }),
      call: vi.fn(async (method: string) => {
        switch (method) {
          case "run.list":
            return { runs: [] };
          case "debug.start":
            return {
              run: { runId: "d1", name: "app", worktree: null, state: "running", kind: "debug" },
              adapter: "debugpy",
            };
          case "debug.threads":
            return { threads: [{ id: 1, name: "main" }] };
          case "debug.stack":
            return {
              frames: [{ id: 1, name: "main", path: PATH, line: 3, column: 1, token: null }],
              totalFrames: null,
            };
          default:
            return { scopes: [] };
        }
      }),
    } as never;
    const openFile = vi.fn();
    const deps = { client, projectId: "r1", openFile, openOutside: vi.fn() };
    const sessions = new DebugSessions();
    sessions.setDeps({ ...deps, machine: MINI });
    sessions.connect(client, "r1");
    await sessions.start("dev");
    expect(sessions.store.getState().sessions[0]?.machine).toBe(MINI);
    listener!({
      topic: "debug.stopped",
      payload: { runId: "d1", threadId: 1, reason: "breakpoint", description: null, session: null },
    });
    await vi.waitFor(() => expect(openFile).toHaveBeenCalled());
    // 창이 다른 머신의 연결로 넘어가는 중(다시 붙기 전)에 사람이 그 프레임을 누른다.
    sessions.setDeps({ ...deps, machine: LOCAL_MACHINE });
    openFile.mockClear();
    await sessions.selectFrame(1);
    expect(openFile.mock.calls[0]![0]).toMatchObject({ path: PATH, machine: MINI });
    expect(sessions.current.getState().frame).toEqual({ path: qualify(MINI, PATH), line: 3 });
  });
});

/** 가짜 디버그 연결 하나: 받은 호출을 적고, `debug.start` 는 `gate` 가 풀릴 때까지 답하지 않는다(`before` 사슬을 기다림). */
function fakeClient(frames: unknown[] = []) {
  const calls: { method: string; params: unknown }[] = [];
  let listener: ((e: unknown) => void) | undefined;
  let release: (() => void) | undefined;
  const gate = new Promise<void>((r) => (release = r));
  const client = {
    subscribe: vi.fn(async (_p: unknown, l: (e: unknown) => void) => {
      listener = l;
      return { unsubscribe: async () => {} };
    }),
    call: vi.fn(async (method: string, params: unknown) => {
      calls.push({ method, params });
      switch (method) {
        case "run.list":
          return { runs: [] };
        case "debug.start":
          await gate;
          return {
            run: { runId: "d1", name: "app", worktree: null, state: "running", kind: "debug" },
            adapter: "debugpy",
          };
        case "debug.threads":
          return { threads: [{ id: 1, name: "main" }] };
        case "debug.stack":
          return { frames, totalFrames: null };
        default:
          return { scopes: [] };
      }
    }),
  };
  return {
    client: client as never,
    calls: (method: string) => calls.filter((c) => c.method === method),
    emit: (e: unknown) => listener!(e),
    release: () => release!(),
  };
}

// 리뷰 반영 2 D1: `debug.start` 가 `before` 사슬을 기다리는 사이 활성 체크아웃이 다른 머신으로 바뀌어도, 그 세션은 새 머신의 세션으로
// 기록되지 않고 멈춤 · 스텝이 새 머신 연결로 가지 않는다.
describe("a debug start that outlives a checkout switch", () => {
  it("is not recorded on the machine the window moved to", async () => {
    const mini = fakeClient();
    const here = fakeClient();
    const sessions = new DebugSessions();
    const base = { projectId: "r1", openFile: vi.fn(), openOutside: vi.fn() };
    sessions.setDeps({ ...base, client: mini.client, machine: MINI });
    const off = sessions.connect(mini.client, "r1");
    const starting = sessions.start("dev");
    await vi.waitFor(() => expect(mini.calls("debug.start")).toHaveLength(1));
    // 사람이 이 머신의 체크아웃으로 바꾼다(창이 이 머신 연결로 다시 붙는다).
    off();
    sessions.setDeps({ ...base, projectId: "p1", client: here.client, machine: LOCAL_MACHINE });
    sessions.connect(here.client, "p1");
    mini.release();
    await starting;
    // 이 창에는 그 세션이 없다 — 새 머신 세션으로도, 이 창이 지금 붙지 않은 머신의 세션으로도(그 머신으로 돌아가면 `run.list`).
    expect(sessions.store.getState().sessions).toEqual([]);
    await sessions.stop().catch(() => {});
    await sessions.control("next").catch(() => {});
    expect(here.calls("run.stop")).toHaveLength(0);
    expect(here.calls("debug.control")).toHaveLength(0);
  });

  // 권고 R2: 세션의 머신이 지금 디버그 연결의 머신이 아니면 그 세션의 `debug.*` · `run.stop` 을 이 연결로 보내지 않는다. 권고 R1:
  // 루트 밖 프레임도 그 세션의 머신으로 연다.
  it("sends nothing for a session of another machine and opens its outside frame there", async () => {
    const mini = fakeClient([
      { id: 1, name: "lib", path: "/usr/lib/x.py", line: 2, column: 1, token: "tok-1" },
    ]);
    const here = fakeClient();
    const sessions = new DebugSessions();
    const openOutside = vi.fn();
    const base = { projectId: "r1", openFile: vi.fn(), openOutside };
    sessions.setDeps({ ...base, client: mini.client, machine: MINI });
    sessions.connect(mini.client, "r1");
    mini.release();
    await sessions.start("dev");
    mini.emit({
      topic: "debug.stopped",
      payload: { runId: "d1", threadId: 1, reason: "breakpoint", description: null, session: null },
    });
    await vi.waitFor(() => expect(sessions.store.getState().frames).toHaveLength(1));
    sessions.setDeps({ ...base, client: here.client, machine: LOCAL_MACHINE });
    await sessions.selectFrame(1);
    expect(openOutside).toHaveBeenCalledWith(
      expect.objectContaining({ machine: MINI, token: "tok-1" }),
    );
    await expect(sessions.stop()).rejects.toThrow(/another machine/);
    expect(here.calls("run.stop")).toHaveLength(0);
    expect(here.calls("debug.scopes")).toHaveLength(0);
  });
});
