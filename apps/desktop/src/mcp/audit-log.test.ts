// 「에이전트 활동」 호출 로그 모델(audit-log.ts): 쪽 넘기기, `mcp.activity` 로 맨 위부터 다시 읽기(새 행 · 끝난 선기록 행), 이벤트가
// 몰릴 때 읽기가 쌓이지 않음, 창이 앞으로 올 때 다른 프로세스의 행(carry-notes R8), 처음부터 다시 읽을 때 옛 응답 버리기(지우기 뒤
// 커서).

import type { AuditRow, McpAuditListParams, McpAuditPage } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import { createAuditLog, mergeRows } from "./audit-log";

function row(id: number, extra: Partial<AuditRow> = {}): AuditRow {
  return {
    id,
    at: 1_000 + id,
    session: "s1",
    client: "claude-code 2.1.0",
    projectId: "p1",
    tool: "fs_read",
    args: "{}",
    outcome: "ok",
    detail: null,
    resultSize: 10,
    durationMs: 3,
    approval: null,
    planHash: null,
    ...extra,
  };
}

/** 감사 로그 흉내: `mcp.audit.list` 의 쪽 규칙(최신부터, 커서 = 앞 쪽 마지막 id, 그 프로젝트 + 프로젝트 없는 행). */
class FakeAudit {
  rows: AuditRow[] = [];
  calls: McpAuditListParams[] = [];
  /** 붙잡아 둔 응답. `hold` 가 켜져 있으면 응답을 여기 두고 `release` 로 보낸다. */
  held: (() => void)[] = [];
  hold = false;

  add(...rows: AuditRow[]) {
    this.rows.push(...rows);
  }

  fetch = (params: McpAuditListParams): Promise<McpAuditPage> => {
    this.calls.push(params);
    // 답은 물은 때의 행으로 만든다 — 붙잡아 두면 그 사이 바뀐 것은 모르는 낡은 답이 된다.
    const answer = this.page(params);
    if (!this.hold) return Promise.resolve(answer);
    return new Promise((resolve) => this.held.push(() => resolve(answer)));
  };

  /** 붙잡아 둔 응답을 보낸다. `latestFirst` 면 나중에 물은 것부터 — 낡은 응답이 새 응답 뒤에 닿는 경우. */
  release(latestFirst = false) {
    const held = latestFirst ? this.held.reverse() : this.held;
    this.held = [];
    for (const r of held) r();
  }

  private page(p: McpAuditListParams): McpAuditPage {
    const before = p.cursor == null ? Infinity : Number(p.cursor);
    const matching = this.rows
      .filter((r) => p.projectId == null || r.projectId === p.projectId || r.projectId === null)
      .filter((r) => p.outcome == null || r.outcome === p.outcome)
      .filter((r) => p.tool == null || r.tool === p.tool)
      .filter((r) => r.id < before)
      .sort((a, b) => b.id - a.id);
    const limit = p.limit ?? 100;
    const entries = matching.slice(0, limit);
    const more = matching.length > limit;
    return { entries, nextCursor: more ? String(entries.at(-1)!.id) : null };
  }
}

function range(from: number, to: number, extra: Partial<AuditRow> = {}) {
  const out: AuditRow[] = [];
  for (let id = from; id <= to; id++) out.push(row(id, extra));
  return out;
}

const ids = (rows: readonly AuditRow[]) => rows.map((r) => r.id);

async function opened(audit: FakeAudit, page = 10) {
  const log = createAuditLog({ fetch: audit.fetch, projectId: "p1", page });
  await log.idle();
  return log;
}

describe("mergeRows", () => {
  it("replaces only the range the fetched pages answer for", () => {
    const rows = [row(9), row(8), row(7), row(5)];
    const merged = mergeRows(rows, [row(10), row(8, { outcome: "error" })], 8, Infinity);
    expect(ids(merged)).toEqual([10, 8, 7, 5]);
    expect(merged[1]!.outcome).toBe("error");
  });
});

describe("audit log", () => {
  it("reads newest first page by page without overlap until the end", async () => {
    const audit = new FakeAudit();
    audit.add(...range(1, 25));
    const log = await opened(audit);
    expect(ids(log.store.getState().rows)).toEqual(
      range(16, 25)
        .map((r) => r.id)
        .reverse(),
    );
    log.loadMore();
    await log.idle();
    log.loadMore();
    await log.idle();
    const { rows, end } = log.store.getState();
    expect(ids(rows)).toEqual(
      range(1, 25)
        .map((r) => r.id)
        .reverse(),
    );
    expect(end).toBe(true);
    expect(audit.calls.map((c) => c.cursor)).toEqual([null, "16", "6"]);
    expect(audit.calls.every((c) => c.projectId === "p1" && c.limit === 10)).toBe(true);
    // 끝에서는 더 읽지 않는다.
    log.loadMore();
    await log.idle();
    expect(audit.calls).toHaveLength(3);
  });

  it("puts a new row on top and updates a started row in place when its end comes", async () => {
    const audit = new FakeAudit();
    audit.add(...range(1, 5), row(6, { outcome: "started", durationMs: 0 }));
    const log = await opened(audit);
    audit.add(row(7, { tool: "git_status" }));
    log.activity([7]);
    await log.idle();
    expect(ids(log.store.getState().rows)).toEqual([7, 6, 5, 4, 3, 2, 1]);

    // 선기록된 행이 끝났다 — 같은 id 로 이벤트가 한 번 더 온다.
    audit.rows[5] = row(6, { outcome: "ok", durationMs: 40 });
    log.activity([6]);
    await log.idle();
    const rows = log.store.getState().rows;
    expect(ids(rows)).toEqual([7, 6, 5, 4, 3, 2, 1]);
    expect(rows[1]).toMatchObject({ outcome: "ok", durationMs: 40 });
  });

  it("reads again once per burst of events, down to the smallest id", async () => {
    const audit = new FakeAudit();
    audit.add(...range(1, 30));
    const log = await opened(audit);
    const before = audit.calls.length;
    audit.hold = true;
    audit.add(row(31));
    log.activity([31]);
    // 첫 새로 읽기가 떠 있는 동안 몰린 이벤트는 하나로 모인다.
    audit.add(row(32), row(33));
    audit.rows[24] = row(25, { outcome: "error" });
    log.activity([32]);
    log.activity([33, 25]);
    log.activity([32]);
    audit.hold = false;
    audit.release();
    await log.idle();
    // 첫 읽기 한 번 + 모은 읽기 한 번(id 25 까지 — 한 쪽).
    expect(audit.calls.length - before).toBe(2);
    const rows = log.store.getState().rows;
    expect(ids(rows).slice(0, 3)).toEqual([33, 32, 31]);
    expect(rows.find((r) => r.id === 25)?.outcome).toBe("error");
  });

  it("does not read below the rows it already holds on an event for an older row", async () => {
    const audit = new FakeAudit();
    audit.add(...range(1, 100));
    const log = await opened(audit);
    const before = audit.calls.length;
    log.activity([3]);
    await log.idle();
    // 받아 둔 행은 91~100 — 3 까지 내려가지 않고 한 쪽만 읽는다.
    expect(audit.calls.length - before).toBe(1);
    expect(log.store.getState().rows).toHaveLength(10);
  });

  it("picks up rows another process wrote and ends of started rows when the window comes forward (R8)", async () => {
    const audit = new FakeAudit();
    audit.add(
      ...range(1, 20),
      row(21, { outcome: "started", client: "pitwall-cli", durationMs: 0 }),
    );
    audit.add(...range(22, 40));
    const log = await opened(audit, 10);
    log.loadMore();
    await log.idle();
    // 받아 둔 것: 21~40. 21 은 아직 started.
    expect(log.store.getState().rows.at(-1)?.id).toBe(21);
    // 이벤트 없이(다른 프로세스) 새 행 두 개와 21 의 끝이 쓰였다.
    audit.add(row(41, { client: "pitwall-cli", tool: "git_status" }), row(42));
    audit.rows[20] = row(21, { outcome: "ok", client: "pitwall-cli", durationMs: 9 });
    const before = audit.calls.length;
    // 21 이 화면에 보인다.
    log.refreshAll([22, 21]);
    await log.idle();
    const rows = log.store.getState().rows;
    expect(ids(rows).slice(0, 2)).toEqual([42, 41]);
    expect(rows.find((r) => r.id === 21)?.outcome).toBe("ok");
    expect(new Set(ids(rows)).size).toBe(rows.length);
    // 첫 쪽 하나와 21 을 id 로 한 번(그 바로 위를 커서로 한 행).
    expect(audit.calls.slice(before).map((c) => [c.cursor, c.limit])).toEqual([
      [null, 10],
      ["22", 1],
    ]);
  });

  it("does not chase a started row that is not on screen when the window comes forward (F3)", async () => {
    const audit = new FakeAudit();
    // 1,000행 중 id 5 가 끝을 못 쓴 채 남았다.
    audit.add(...range(1, 4), row(5, { outcome: "started", durationMs: 0 }), ...range(6, 1_000));
    const log = await opened(audit, 200);
    for (let i = 0; i < 5; i++) {
      log.loadMore();
      await log.idle();
    }
    expect(log.store.getState()).toMatchObject({ end: true });
    expect(log.store.getState().rows).toHaveLength(1_000);
    const before = audit.calls.length;
    // 맨 위를 보고 있다 — 5 는 화면에 없다.
    log.refreshAll(range(985, 1_000).map((r) => r.id));
    await log.idle();
    // 예전에는 5 까지 맨 위부터 다섯 쪽(1,000행)을 다시 읽었다.
    expect(audit.calls.length - before).toBe(1);
  });

  it("drops a started row on recheck when it no longer matches the filter", async () => {
    const audit = new FakeAudit();
    audit.add(...range(1, 3), row(4, { outcome: "started", durationMs: 0 }), ...range(5, 30));
    const log = await opened(audit, 10);
    log.setFilter({ outcome: "started", tool: null });
    await log.idle();
    expect(ids(log.store.getState().rows)).toEqual([4]);
    audit.rows[3] = row(4, { outcome: "ok" });
    log.refreshAll([4]);
    await log.idle();
    expect(ids(log.store.getState().rows)).toEqual([]);
  });

  it("drops a page that left before the log was read again from the start (clearing)", async () => {
    const audit = new FakeAudit();
    audit.add(...range(1, 30));
    audit.add(row(31, { projectId: null }));
    const log = await opened(audit);
    audit.hold = true;
    log.loadMore();
    // 다음 쪽이 떠 있는 사이 이 프로젝트의 기록을 지우고 처음부터 다시 읽는다.
    audit.rows = audit.rows.filter((r) => r.projectId === null);
    log.reset();
    audit.hold = false;
    audit.release(true);
    await log.idle();
    const { rows, end } = log.store.getState();
    expect(ids(rows)).toEqual([31]);
    expect(end).toBe(true);
    // 지운 뒤 새 행은 더 큰 id 다(M5-11) — 맨 위에 붙고, 옛 커서의 쪽은 붙지 않았다.
    audit.add(row(32));
    log.activity([32]);
    await log.idle();
    expect(ids(log.store.getState().rows)).toEqual([32, 31]);
  });

  it("drops a refresh that left before the log was read again from the start (clearing)", async () => {
    const audit = new FakeAudit();
    audit.add(...range(1, 5), row(6, { projectId: null }));
    const log = await opened(audit);
    audit.hold = true;
    audit.add(row(7));
    log.activity([7]);
    // 새로 읽기가 떠 있는 사이 지우고 처음부터. 낡은 새로 읽기의 답이 새 첫 쪽보다 늦게 닿는다.
    audit.rows = audit.rows.filter((r) => r.projectId === null);
    log.reset();
    audit.hold = false;
    audit.release(true);
    await log.idle();
    expect(ids(log.store.getState().rows)).toEqual([6]);
  });

  it("asks the host to filter by result and tool, and starts over when they change", async () => {
    const audit = new FakeAudit();
    audit.add(...range(1, 5), row(6, { outcome: "denied", tool: "db_execute" }));
    const log = await opened(audit);
    log.setFilter({ outcome: "denied", tool: null });
    await log.idle();
    expect(ids(log.store.getState().rows)).toEqual([6]);
    expect(audit.calls.at(-1)).toMatchObject({ outcome: "denied", tool: null, cursor: null });
    log.setFilter({ outcome: null, tool: "fs_read" });
    await log.idle();
    expect(ids(log.store.getState().rows)).toEqual([5, 4, 3, 2, 1]);
    expect(audit.calls.at(-1)).toMatchObject({ outcome: null, tool: "fs_read" });
    // 거르기에 더는 맞지 않게 된 행은 다시 읽을 때 빠진다.
    log.setFilter({ outcome: "ok", tool: null });
    await log.idle();
    audit.rows[4] = row(5, { outcome: "error" });
    log.activity([5]);
    await log.idle();
    expect(ids(log.store.getState().rows)).toEqual([4, 3, 2, 1]);
  });

  it("keeps the error of a failed read and clears it on the next good one", async () => {
    let fail = true;
    const audit = new FakeAudit();
    audit.add(row(1));
    const log = createAuditLog({
      projectId: "p1",
      fetch: (p) => (fail ? Promise.reject(new Error("disk")) : audit.fetch(p)),
    });
    await log.idle();
    expect(log.store.getState()).toMatchObject({ error: "disk", loading: false, rows: [] });
    fail = false;
    log.refreshAll();
    await log.idle();
    expect(log.store.getState()).toMatchObject({ error: undefined, loading: false });
    expect(ids(log.store.getState().rows)).toEqual([1]);
  });
});
