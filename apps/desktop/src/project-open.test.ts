import { describe, expect, it, vi } from "vitest";

import {
  describeOpen,
  type ProjectOpen,
  type ProjectOpenSource,
  projectIdFromQuery,
  watchProjectOpens,
} from "./project-open";

/** Rust 쪽 큐를 흉내 낸다. `listen` 이 끝나기 전까지 알림은 아무에게도 가지 않는다(Tauri 는 쌓아 두지 않는다). */
function fakeSource() {
  const queue: ProjectOpen[] = [];
  let notify: (() => void) | undefined;
  const calls: string[] = [];
  let finishListen!: () => void;
  const listened = new Promise<void>((resolve) => (finishListen = resolve));
  const source: ProjectOpenSource = {
    async listen(onNotify) {
      calls.push("listen");
      await listened;
      notify = onNotify;
      return () => (notify = undefined);
    },
    take: vi.fn(async () => {
      calls.push("take");
      return queue.splice(0);
    }),
  };
  return {
    source,
    calls,
    finishListen,
    /** Rust 의 `open_project`: 큐에 넣고 알린다. */
    push(open: ProjectOpen) {
      queue.push(open);
      notify?.();
    },
  };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("projectIdFromQuery", () => {
  it("reads the project id Rust puts in a project window's query", () => {
    expect(projectIdFromQuery("?window=project&project=ab12")).toBe("ab12");
    expect(projectIdFromQuery("?window=project")).toBeUndefined();
  });
});

describe("watchProjectOpens", () => {
  it("keeps requests that arrived before the listener and delivers them in order", async () => {
    const fake = fakeSource();
    const got: ProjectOpen[] = [];
    watchProjectOpens(fake.source, (o) => got.push(o));
    // 창을 만드는 중: 리스너가 아직 없어 알림은 사라지고 요청은 큐에 남는다.
    fake.push({ file: "/r/a.rs" });
    fake.push({ file: "/r/b.rs", line: 2 });
    fake.finishListen();
    await settle();
    expect(got).toEqual([{ file: "/r/a.rs" }, { file: "/r/b.rs", line: 2 }]);
    expect(fake.calls.slice(0, 2)).toEqual(["listen", "take"]);
    // 리스너가 선 뒤의 요청은 알림이 부르는 꺼내기가 받는다.
    fake.push({ worktree: "/wt" });
    await settle();
    expect(got).toEqual([{ file: "/r/a.rs" }, { file: "/r/b.rs", line: 2 }, { worktree: "/wt" }]);
  });

  it("stops taking after it is disposed", async () => {
    const fake = fakeSource();
    const got: ProjectOpen[] = [];
    const stop = watchProjectOpens(fake.source, (o) => got.push(o));
    fake.finishListen();
    await settle();
    // 알림이 꺼내기를 예약한 직후(같은 틱)에 해제해도 꺼내지 않는다 — 큐는 다음 창(같은 label)이 받는다.
    fake.push({ file: "/r/a.rs" });
    stop();
    await settle();
    expect(got).toEqual([]);
    expect(fake.source.take).toHaveBeenCalledTimes(1);
  });
});

describe("describeOpen", () => {
  it("names the worktree and the file location", () => {
    expect(describeOpen({ worktree: "/wt", file: "/wt/a.rs", line: 3, col: 4 })).toBe(
      "worktree /wt, file /wt/a.rs:3:4",
    );
    expect(describeOpen({ file: "/a.rs", line: 7 })).toBe("file /a.rs:7");
    expect(describeOpen({})).toBeUndefined();
  });
});
