import type { UiLogBatch } from "@pitwall/rpc-client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { UiLogger, captureGlobalErrors, describeError } from "../src";

function recorder() {
  const batches: UiLogBatch[] = [];
  const pending: Array<{ resolve: () => void; reject: (e: unknown) => void }> = [];
  const send = vi.fn(
    (batch: UiLogBatch) =>
      new Promise<void>((resolve, reject) => {
        batches.push(batch);
        pending.push({ resolve, reject });
      }),
  );
  const settle = async (ok = true) => {
    const next = pending.shift();
    if (!next) throw new Error("no batch in flight");
    if (ok) next.resolve();
    else next.reject(new Error("ipc down"));
    await vi.advanceTimersByTimeAsync(0);
  };
  return { batches, send, settle, inFlight: () => pending.length };
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("UiLogger", () => {
  it("sends after the flush interval as one batch", async () => {
    const r = recorder();
    const log = new UiLogger({ send: r.send, flushIntervalMs: 1000 });
    log.info("a", "main");
    log.error("b");
    await vi.advanceTimersByTimeAsync(999);
    expect(r.send).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(r.batches).toEqual([
      {
        entries: [
          { level: "info", message: "a", source: "main" },
          { level: "error", message: "b", source: null },
        ],
        dropped: 0,
      },
    ]);
  });

  it("sends at once when a batch fills, and keeps one batch in flight", async () => {
    const r = recorder();
    const log = new UiLogger({ send: r.send, maxBatch: 2 });
    log.info("1");
    log.info("2");
    log.info("3");
    log.info("4");
    await vi.advanceTimersByTimeAsync(0);
    expect(r.inFlight()).toBe(1);
    expect(r.batches.map((b) => b.entries.map((e) => e.message))).toEqual([["1", "2"]]);
    await r.settle();
    expect(r.batches.map((b) => b.entries.map((e) => e.message))).toEqual([
      ["1", "2"],
      ["3", "4"],
    ]);
  });

  it("drops the oldest entries past the queue limit and reports the count", async () => {
    const r = recorder();
    const log = new UiLogger({ send: r.send, maxBatch: 100, maxQueue: 3 });
    for (const m of ["1", "2", "3", "4", "5"]) log.warn(m);
    void log.flush();
    await vi.advanceTimersByTimeAsync(0);
    expect(r.batches[0]?.entries.map((e) => e.message)).toEqual(["3", "4", "5"]);
    expect(r.batches[0]?.dropped).toBe(2);
  });

  it("counts a failed batch as dropped and does not resend it", async () => {
    const r = recorder();
    const log = new UiLogger({ send: r.send, flushIntervalMs: 10 });
    log.error("lost");
    await vi.advanceTimersByTimeAsync(10);
    await r.settle(false);
    expect(r.inFlight()).toBe(0);
    log.error("next");
    await vi.advanceTimersByTimeAsync(10);
    expect(r.batches[1]).toEqual({
      entries: [{ level: "error", message: "next", source: null }],
      dropped: 1,
    });
  });

  it("filters by level and truncates long messages", async () => {
    const r = recorder();
    const log = new UiLogger({ send: r.send, minLevel: "warn", maxMessageLength: 5 });
    log.debug("quiet");
    log.info("quiet");
    log.warn("0123456789");
    void log.flush();
    await vi.advanceTimersByTimeAsync(0);
    expect(r.batches[0]?.entries).toEqual([
      { level: "warn", message: "01234…(truncated)", source: null },
    ]);
  });
});

describe("captureGlobalErrors", () => {
  it("logs window errors and unhandled rejections, and detaches", () => {
    const target = new EventTarget();
    const errors: Array<[string, string | undefined]> = [];
    const detach = captureGlobalErrors(target, {
      error: (message, source) => errors.push([message, source]),
    });

    const boom = new TypeError("boom");
    target.dispatchEvent(Object.assign(new Event("error"), { error: boom, message: "boom" }));
    target.dispatchEvent(
      Object.assign(new Event("error"), { error: null, message: "Script error." }),
    );
    target.dispatchEvent(Object.assign(new Event("unhandledrejection"), { reason: "nope" }));
    detach();
    target.dispatchEvent(Object.assign(new Event("unhandledrejection"), { reason: "late" }));

    expect(errors).toEqual([
      [describeError(boom), "window.error"],
      ["Script error.", "window.error"],
      ["nope", "unhandledrejection"],
    ]);
    expect(errors[0]?.[0]).toContain("TypeError: boom");
  });

  it("describes non-error values", () => {
    expect(describeError({ code: 7 })).toBe('{"code":7}');
    expect(describeError(undefined)).toBe("undefined");
  });
});
