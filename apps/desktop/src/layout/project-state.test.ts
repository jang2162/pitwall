// 프로젝트 상태(project-state.ts): 읽기 결과에 따른 쓰기 여부, 모아 쓰기, 옛 레이아웃 옮기기, 묶음 읽기 · 버리기.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { LAYOUT_STORAGE_KEY, type LayoutStorage } from "./project-layout";
import {
  createProjectState,
  MAX_WORKTREES,
  readStateFile,
  type StateBackend,
  STATE_VERSION,
} from "./project-state";

function memoryBackend(initial: unknown = null, problem: string | null = null) {
  const sets: unknown[] = [];
  const backend: StateBackend & { sets: unknown[]; fail?: boolean } = {
    sets,
    get: vi.fn(async () => ({ state: initial, problem })),
    set: vi.fn(async (state) => {
      if (backend.fail) throw new Error("disk full");
      sets.push(structuredClone(state));
    }),
  };
  return backend;
}

function memoryStorage(entries: Record<string, string> = {}): LayoutStorage & {
  map: Map<string, string>;
} {
  const map = new Map(Object.entries(entries));
  return {
    map,
    getItem: (k) => map.get(k) ?? null,
    setItem: (k, v) => void map.set(k, v),
    removeItem: (k) => void map.delete(k),
  };
}

const quiet = () => {};

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("createProjectState", () => {
  it("writes changes once after the delay and on flush, and only what changed", async () => {
    const backend = memoryBackend();
    const state = createProjectState({ backend, delayMs: 100, log: quiet });
    expect(await state.load()).toBe("empty");
    state.layout.setItem(LAYOUT_STORAGE_KEY, JSON.stringify({ version: 1, a: 1 }));
    state.setBundle(undefined, { tabs: [{ path: "/p/a.rs" }] });
    expect(backend.sets).toEqual([]);
    await vi.advanceTimersByTimeAsync(100);
    expect(backend.sets).toHaveLength(1);
    expect(backend.sets[0]).toMatchObject({
      version: STATE_VERSION,
      layout: { version: 1, a: 1 },
      main: { tabs: [{ path: "/p/a.rs" }] },
    });
    // 바뀐 것이 없으면 flush 해도 쓰지 않는다.
    await state.flush();
    expect(backend.sets).toHaveLength(1);
    state.setBundle("/w1", { tabs: [] });
    await state.flush();
    expect(backend.sets).toHaveLength(2);
  });

  it("never writes from a window that could not read the saved state", async () => {
    const backend = memoryBackend();
    backend.get = vi.fn(async () => {
      throw new Error("no project.state.get");
    });
    const legacy = memoryStorage({ [LAYOUT_STORAGE_KEY]: JSON.stringify({ version: 1 }) });
    const state = createProjectState({ backend, legacy, log: quiet });
    expect(await state.load()).toBe("unavailable");
    state.setBundle(undefined, { tabs: [{ path: "/p/a.rs" }] });
    await state.flush();
    await vi.advanceTimersByTimeAsync(1000);
    expect(backend.sets).toEqual([]);
    // 옮기지도 지우지도 않는다.
    expect(legacy.map.has(LAYOUT_STORAGE_KEY)).toBe(true);
  });

  it("starts over from a broken or other-version state and overwrites it on the next write", async () => {
    for (const [saved, problem] of [
      [null, "state.json is not JSON"],
      [{ version: 99, layout: {} }, null],
      [[1, 2], null],
    ] as const) {
      const backend = memoryBackend(saved, problem);
      const state = createProjectState({ backend, log: quiet });
      expect(await state.load()).toBe("broken");
      expect(state.layout.getItem(LAYOUT_STORAGE_KEY)).toBeNull();
      expect(state.bundle(undefined)).toBeUndefined();
      state.setBundle(undefined, { tabs: [] });
      await state.flush();
      expect(backend.sets[0]).toMatchObject({ version: STATE_VERSION, main: { tabs: [] } });
    }
  });

  it("moves the old localStorage layout once, removing it only after the write succeeds", async () => {
    const old = { version: 1, layout: { panels: {} }, hidden: { project: "left" } };
    const legacy = memoryStorage({ [LAYOUT_STORAGE_KEY]: JSON.stringify(old) });
    const backend = memoryBackend();
    backend.fail = true;
    const state = createProjectState({ backend, legacy, log: quiet });
    await state.load();
    expect(JSON.parse(state.layout.getItem(LAYOUT_STORAGE_KEY)!)).toEqual(old);
    await state.flush();
    expect(legacy.map.has(LAYOUT_STORAGE_KEY)).toBe(true);

    backend.fail = false;
    state.layout.setItem(LAYOUT_STORAGE_KEY, JSON.stringify(old));
    await state.flush();
    expect(backend.sets).toEqual([expect.objectContaining({ layout: old })]);
    expect(legacy.map.has(LAYOUT_STORAGE_KEY)).toBe(false);
  });

  it("leaves the old layout alone when the project already has its own", async () => {
    const legacy = memoryStorage({ [LAYOUT_STORAGE_KEY]: JSON.stringify({ version: 1, x: 1 }) });
    const backend = memoryBackend({ version: STATE_VERSION, layout: { version: 1, own: true } });
    const state = createProjectState({ backend, legacy, log: quiet });
    expect(await state.load()).toBe("loaded");
    expect(JSON.parse(state.layout.getItem(LAYOUT_STORAGE_KEY)!)).toEqual({
      version: 1,
      own: true,
    });
    await state.flush();
    expect(backend.sets).toEqual([]);
    expect(legacy.map.has(LAYOUT_STORAGE_KEY)).toBe(true);
  });

  it("keeps only the most recently used worktree tab sets, and always the main checkout", async () => {
    let clock = 0;
    const state = createProjectState({ backend: memoryBackend(), now: () => ++clock, log: quiet });
    await state.load();
    state.setBundle(undefined, { tabs: [] });
    for (let i = 0; i < MAX_WORKTREES + 3; i++) state.setBundle(`/w${i}`, { tabs: [] });
    const kept = Object.keys(state.snapshot().worktrees ?? {});
    expect(kept).toHaveLength(MAX_WORKTREES);
    expect(kept).not.toContain("/w0");
    expect(kept).toContain(`/w${MAX_WORKTREES + 2}`);
    expect(state.bundle(undefined)).toBeDefined();
  });

  it("keeps the window's recent files apart from the worktree tab sets", async () => {
    const backend = memoryBackend();
    const state = createProjectState({ backend, log: quiet });
    await state.load();
    state.setRecentFiles([{ path: "/w1/a.rs", worktree: "/w1" }, { path: "/m/b.rs" }]);
    state.setBundle("/w1", { tabs: [{ path: "/w1/c.rs" }] });
    expect(state.recentFiles()).toEqual([
      { path: "/w1/a.rs", worktree: "/w1" },
      { path: "/m/b.rs" },
    ]);
    await state.flush();
    const saved = backend.sets[0];
    expect(readStateFile(saved)?.recentFiles).toEqual(state.recentFiles());
    expect(
      readStateFile({ ...(saved as object), recentFiles: [{ path: 3 }, "x", { path: "/k" }] })
        ?.recentFiles,
    ).toEqual([{ path: "/k" }]);
  });

  it("gives up on a read that does not answer in time and never writes, even when it answers later", async () => {
    const backend = memoryBackend({ version: STATE_VERSION, layout: { saved: true } });
    let answer: (v: { state: unknown; problem: null }) => void = () => {};
    backend.get = vi.fn(
      () => new Promise<{ state: unknown; problem: null }>((resolve) => (answer = resolve)),
    );
    const state = createProjectState({ backend, delayMs: 10, loadLimitMs: 50, log: quiet });
    const loaded = state.load();
    await vi.advanceTimersByTimeAsync(50);
    expect(await loaded).toBe("unavailable");
    answer({ state: { version: STATE_VERSION, layout: { saved: true } }, problem: null });
    state.setBundle(undefined, { tabs: [{ path: "/a" }] });
    await vi.advanceTimersByTimeAsync(10);
    await state.flush();
    expect(backend.set).not.toHaveBeenCalled();
    expect(state.snapshot().layout).toBeUndefined();
  });

  it("opens with defaults and never writes when it gives up before reaching the Host", async () => {
    const backend = memoryBackend();
    const state = createProjectState({ backend, delayMs: 10, log: quiet });
    expect(await state.giveUp()).toBe("unavailable");
    expect(await state.load()).toBe("unavailable");
    expect(backend.get).not.toHaveBeenCalled();
    state.setBundle(undefined, { tabs: [{ path: "/a" }] });
    await state.flush();
    expect(backend.set).not.toHaveBeenCalled();
  });

  it("stops waiting for a write that does not answer when a limit is given (closing the window)", async () => {
    const backend = memoryBackend();
    backend.set = vi.fn(() => new Promise<void>(() => {}));
    const state = createProjectState({ backend, log: quiet });
    await state.load();
    state.setBundle(undefined, { tabs: [{ path: "/a" }] });
    let done = false;
    void state.flush(100).then(() => (done = true));
    await vi.advanceTimersByTimeAsync(99);
    expect(done).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(done).toBe(true);
    expect(backend.set).toHaveBeenCalledTimes(1);
  });

  it("loads once even when asked twice (StrictMode)", async () => {
    const backend = memoryBackend();
    const state = createProjectState({ backend, log: quiet });
    await Promise.all([state.load(), state.load()]);
    expect(backend.get).toHaveBeenCalledTimes(1);
  });
});

describe("readStateFile", () => {
  it("drops tabs and fields it cannot read, keeping the rest", () => {
    expect(
      readStateFile({
        version: STATE_VERSION,
        main: {
          tabs: [
            { path: "/a", cursor: { line: 3, column: 2 }, top: 10, pinned: true },
            { path: "" },
            { nope: 1 },
            { path: "/b", cursor: { line: "x" }, preview: true },
            { path: "/c", worktree: "/w1" },
            { path: "/d", worktree: null },
            { path: "/e", worktree: 5 },
          ],
          active: 1,
          tree: { root: "/r", expanded: ["/r/x", 5] },
          usedAt: 7,
        },
        worktrees: { "/w": { tabs: "nope" }, "/v": { tabs: [] } },
      }),
    ).toEqual({
      version: STATE_VERSION,
      main: {
        tabs: [
          { path: "/a", cursor: { line: 3, column: 2 }, top: 10, pinned: true },
          { path: "/b", preview: true },
          { path: "/c", worktree: "/w1" },
          { path: "/d", worktree: null },
          { path: "/e" },
        ],
        active: 1,
        tree: { root: "/r", expanded: ["/r/x"] },
        usedAt: 7,
      },
      worktrees: { "/v": { tabs: [], usedAt: 0 } },
    });
    expect(readStateFile({ version: 2 })).toBeUndefined();
    expect(readStateFile("x")).toBeUndefined();
  });
});
