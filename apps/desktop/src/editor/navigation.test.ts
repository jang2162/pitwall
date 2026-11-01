// 내비게이션 기록(navigation.ts, 15-m4-plan M4-09)의 모양: 뒤로 · 앞으로가 떠난 자리로 돌아가고, 상한 100 을 넘으면 가장 오래된
// 것이 빠지고, 편집이 기록 위치를 옮기고, 열지 못한 기록 · 다른 워크트리의 기록을 건너뛰고, 큰 이동만 기록한다. 화면(키 · 마우스 ·
// 팝업)은 navigation-window.test.tsx 가 본다.

import { ChangeSet, Text } from "@codemirror/state";
import { describe, expect, it } from "vitest";

import type { DocumentChange } from "./documents";
import {
  BIG_MOVE_LINES,
  createNavigation,
  mapLocation,
  NAV_HISTORY_LIMIT,
  type NavLocation,
  type NavTarget,
} from "./navigation";

/** 가짜 창: 지금 자리 하나와, 열 수 없는 경로 · 바뀜을 보내는 문서 모델. */
function harness(init: NavLocation = { path: "/a", line: 1, column: 1 }) {
  let here: NavLocation | undefined = init;
  const gone = new Set<string>();
  const opened: NavTarget[] = [];
  /** 열 때마다 뒤로 · 앞으로의 열기였는가(`travel`). */
  const travels: boolean[] = [];
  const listeners = new Map<string, Set<(c: DocumentChange) => void>>();
  let worktree: string | undefined;
  const nav = createNavigation({
    documents: {
      onChange(path, listener) {
        const set = listeners.get(path) ?? new Set();
        listeners.set(path, set);
        set.add(listener);
        return () => set.delete(listener);
      },
    },
    here: () => here,
    open: async (loc, how) => {
      opened.push(loc);
      travels.push(how.travel);
      if (gone.has(loc.path)) return false;
      here = { ...loc, line: loc.line ?? here?.line ?? 1, column: loc.column ?? 1 };
      return true;
    },
    activeWorktree: () => worktree,
  });
  return {
    nav,
    gone,
    opened,
    travels,
    here: () => here,
    /** 사람이 커서를 옮겼다(기록하지 않는 이동). */
    moveTo(loc: NavLocation) {
      here = loc;
    },
    setWorktree(next: string | undefined) {
      worktree = next;
    },
    /** 문서 `path` 를 편집했다. */
    edit(path: string, before: string, changes: { from: number; to?: number; insert?: string }) {
      const a = Text.of(before.split("\n"));
      const set = ChangeSet.of(changes, a.length);
      const after = set.apply(a);
      for (const l of listeners.get(path) ?? [])
        l({ kind: "edit", changes: set, before: a, after });
    },
    watching: () => [...listeners].filter(([, s]) => s.size > 0).map(([p]) => p),
  };
}

const at = (path: string, line: number, column = 1): NavLocation => ({ path, line, column });

describe("navigation history", () => {
  it("goes back to where a recorded jump left and forward to where it went", async () => {
    const h = harness(at("/a", 5, 3));
    await h.nav.navigateTo(at("/b", 40, 2), { record: true });
    expect(h.here()).toEqual(at("/b", 40, 2));

    expect(await h.nav.back()).toBe(true);
    expect(h.here()).toEqual(at("/a", 5, 3));
    expect(await h.nav.forward()).toBe(true);
    expect(h.here()).toEqual(at("/b", 40, 2));
    // 끝에서는 더 가지 않는다.
    expect(await h.nav.forward()).toBe(false);
    expect(h.here()).toEqual(at("/b", 40, 2));
  });

  it("does not record without `record`, nor when the open fails", async () => {
    const h = harness(at("/a", 5));
    await h.nav.navigateTo(at("/b", 1));
    expect(h.nav.entries()).toEqual([]);
    h.gone.add("/c");
    expect(await h.nav.navigateTo(at("/c", 1), { record: true })).toBe(false);
    expect(h.nav.entries()).toEqual([]);
    expect(h.here()).toEqual(at("/b", 1));
  });

  it("drops the forward side on a new record", async () => {
    const h = harness(at("/a", 1));
    await h.nav.navigateTo(at("/b", 1), { record: true });
    await h.nav.navigateTo(at("/c", 1), { record: true });
    await h.nav.back();
    expect(h.here()).toEqual(at("/b", 1));
    await h.nav.navigateTo(at("/d", 1), { record: true });
    expect(h.nav.entries()).toEqual([at("/a", 1), at("/b", 1)]);
    expect(await h.nav.forward()).toBe(false);
  });

  it(`keeps ${NAV_HISTORY_LIMIT} entries: the 101st record drops the oldest`, async () => {
    const h = harness(at("/f0", 1));
    for (let i = 1; i <= NAV_HISTORY_LIMIT + 1; i++)
      await h.nav.navigateTo(at(`/f${i}`, 1), { record: true });
    const entries = h.nav.entries();
    expect(entries).toHaveLength(NAV_HISTORY_LIMIT);
    expect(entries[0]).toEqual(at("/f1", 1));
    expect(entries.at(-1)).toEqual(at(`/f${NAV_HISTORY_LIMIT}`, 1));
  });

  it("caps back and forward together when going back adds the current place", async () => {
    const h = harness(at("/f0", 1));
    for (let i = 1; i <= NAV_HISTORY_LIMIT; i++)
      await h.nav.navigateTo(at(`/f${i}`, 1), { record: true });
    expect(h.nav.entries()).toHaveLength(NAV_HISTORY_LIMIT);
    await h.nav.back();
    expect(h.nav.entries()).toHaveLength(NAV_HISTORY_LIMIT);
  });

  it("moves an entry with the edits of its document and stops listening once the path leaves", async () => {
    const text = Array.from({ length: 30 }, (_, i) => `line ${i + 1}`).join("\n");
    const h = harness(at("/a", 20, 3));
    await h.nav.navigateTo(at("/b", 1), { record: true });
    expect(h.watching()).toEqual(["/a"]);
    // 위쪽 다섯 줄(1~5줄)을 지운다.
    const lineFrom = (n: number) =>
      text
        .split("\n")
        .slice(0, n - 1)
        .join("\n").length + 1;
    h.edit("/a", text, { from: 0, to: lineFrom(6) });
    expect(h.nav.entries()).toEqual([at("/a", 15, 3)]);
    // 다른 문서의 편집은 건드리지 않는다.
    h.edit("/c", text, { from: 0, to: lineFrom(6) });
    expect(h.nav.entries()).toEqual([at("/a", 15, 3)]);

    await h.nav.back();
    expect(h.here()).toEqual(at("/a", 15, 3));
    // 기록에는 이제 /b 만 있다.
    expect(h.watching()).toEqual(["/b"]);
  });

  it("goes on past an entry it cannot open (a deleted file) and drops it", async () => {
    const h = harness(at("/a", 1));
    await h.nav.navigateTo(at("/b", 1), { record: true });
    await h.nav.navigateTo(at("/c", 1), { record: true });
    h.gone.add("/b");
    expect(await h.nav.back()).toBe(true);
    expect(h.here()).toEqual(at("/a", 1));
    expect(h.nav.entries().map((e) => e.path)).toEqual(["/c"]);
  });

  it("skips entries of another worktree and the current line", async () => {
    const h = harness({ path: "/w1/a", worktree: "/w1", line: 1, column: 1 });
    h.setWorktree("/w1");
    await h.nav.navigateTo({ path: "/w1/b", worktree: "/w1", line: 1 }, { record: true });
    h.setWorktree(undefined);
    h.moveTo(at("/main/x", 1));
    await h.nav.navigateTo(at("/main/y", 9), { record: true });
    expect(h.nav.entries().map((e) => e.path)).toEqual(["/w1/a", "/main/x"]);
    await h.nav.back();
    expect(h.here()).toEqual(at("/main/x", 1));
    // 남은 뒤로 쪽은 다른 워크트리(/w1)의 것뿐이다.
    expect(await h.nav.back()).toBe(false);
    expect(h.here()).toEqual(at("/main/x", 1));
  });

  it("records a move of more than 10 lines in one tab, not a smaller, quiet or first one", () => {
    const h = harness();
    const tab = { path: "/a" };
    h.nav.cursorMoved("p1", tab, { line: 5, column: 2 }, false);
    // 첫 보고는 비교할 것이 없다.
    expect(h.nav.entries()).toEqual([]);
    h.nav.cursorMoved("p1", tab, { line: 5 + BIG_MOVE_LINES, column: 1 }, false);
    expect(h.nav.entries()).toEqual([]);
    h.nav.cursorMoved("p1", tab, { line: 5 + 2 * BIG_MOVE_LINES + 1, column: 4 }, false);
    expect(h.nav.entries()).toEqual([at("/a", 5 + BIG_MOVE_LINES, 1)]);
    // 편집이 민 커서 · 작업 공간이 둔 커서는 기록하지 않는다.
    h.nav.cursorMoved("p1", tab, { line: 100, column: 1 }, true);
    expect(h.nav.entries()).toHaveLength(1);
    // 다른 탭의 커서와 비교하지 않는다.
    h.nav.cursorMoved("p2", { path: "/b" }, { line: 1, column: 1 }, false);
    expect(h.nav.entries()).toHaveLength(1);
    h.nav.forgetPanel("p1");
    h.nav.cursorMoved("p1", tab, { line: 1, column: 1 }, false);
    expect(h.nav.entries()).toHaveLength(1);
  });

  it("keeps one slot for a run of big moves: the place the run started", () => {
    const h = harness();
    const tab = { path: "/a" };
    h.nav.cursorMoved("p1", tab, { line: 1, column: 1 }, false);
    // PageDown 을 이어 누른 것처럼 40줄씩 세 번.
    for (const line of [41, 81, 121]) h.nav.cursorMoved("p1", tab, { line, column: 1 }, false);
    expect(h.nav.entries()).toEqual([at("/a", 1)]);
    // 작은 이동(화살표)은 연속을 끊지 않는다.
    h.nav.cursorMoved("p1", tab, { line: 122, column: 1 }, false);
    h.nav.cursorMoved("p1", tab, { line: 200, column: 1 }, false);
    expect(h.nav.entries()).toEqual([at("/a", 1)]);
  });

  it("starts a new run after an edit, another record or a move in another tab", async () => {
    const h = harness(at("/z", 1));
    const tab = { path: "/a" };
    h.nav.cursorMoved("p1", tab, { line: 1, column: 1 }, false);
    h.nav.cursorMoved("p1", tab, { line: 50, column: 1 }, false);
    // 편집(quiet 보고)이 끊는다.
    h.nav.cursorMoved("p1", tab, { line: 50, column: 2 }, true);
    h.nav.cursorMoved("p1", tab, { line: 100, column: 1 }, false);
    expect(h.nav.entries()).toEqual([at("/a", 1), at("/a", 50, 2)]);
    // 다른 탭의 커서 이동이 끊는다.
    h.nav.cursorMoved("p2", { path: "/b" }, { line: 5, column: 1 }, false);
    h.nav.cursorMoved("p1", tab, { line: 150, column: 1 }, false);
    expect(h.nav.entries()).toHaveLength(3);
    // 다른 기록(위치로 가기)이 끊는다.
    await h.nav.navigateTo(at("/c", 1), { record: true });
    h.nav.cursorMoved("p1", tab, { line: 200, column: 1 }, false);
    expect(h.nav.entries()).toHaveLength(5);
  });

  it("opens quietly when going back or forward, and loudly for a chosen place", async () => {
    const h = harness(at("/a", 1));
    await h.nav.navigateTo(at("/b", 1), { record: true });
    await h.nav.back();
    await h.nav.forward();
    expect(h.travels).toEqual([false, true, true]);
  });

  it("carries a root-outside token through back and forward", async () => {
    const h = harness({ path: "/lib/vec.rs", token: "t1", line: 9, column: 1 });
    await h.nav.navigateTo(at("/a", 3), { record: true });
    expect(h.nav.entries()).toEqual([{ path: "/lib/vec.rs", token: "t1", line: 9, column: 1 }]);
    await h.nav.back();
    expect(h.opened.at(-1)).toMatchObject({ path: "/lib/vec.rs", token: "t1", line: 9 });
  });

  it("follows a rename of the entry's file or folder", async () => {
    const h = harness(at("/p/src/a.rs", 3));
    await h.nav.navigateTo(at("/p/b.rs", 1), { record: true });
    h.nav.moved(
      (p) => p.startsWith("/p/src/"),
      (p) => `/p/lib/${p.slice("/p/src/".length)}`,
    );
    expect(h.nav.entries()).toEqual([at("/p/lib/a.rs", 3)]);
  });
});

describe("mapLocation", () => {
  const before = Text.of(["aaa", "bbb", "ccc"]);
  const loc = at("/a", 3, 2);

  it("follows the text when a line above is deleted or inserted", () => {
    const del = ChangeSet.of({ from: 0, to: 4 }, before.length);
    expect(mapLocation(loc, del, before, del.apply(before))).toEqual(at("/a", 2, 2));
    const ins = ChangeSet.of({ from: 0, insert: "x\ny\n" }, before.length);
    expect(mapLocation(loc, ins, before, ins.apply(before))).toEqual(at("/a", 5, 2));
  });

  it("stays after text inserted right at it", () => {
    // 「ccc」 의 둘째 글자 앞(위치 9)에 「zz」 를 넣는다 — 기록은 원래 글자(c)를 따라간다.
    const ins = ChangeSet.of({ from: 9, insert: "zz" }, before.length);
    expect(mapLocation(loc, ins, before, ins.apply(before))).toEqual(at("/a", 3, 4));
  });

  it("lands at the start of a deleted range that held it", () => {
    const del = ChangeSet.of({ from: 3, to: before.length }, before.length);
    expect(mapLocation(loc, del, before, del.apply(before))).toEqual(at("/a", 1, 4));
  });
});
