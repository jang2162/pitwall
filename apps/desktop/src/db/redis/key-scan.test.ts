// 키 스캔(key-scan.ts): 스트림 묶음을 트리에 붙이고 `done` 으로 끝, 트리의 보유 상한에서 스트림을 끊고 `capped`, 멈추면
// `stopped`, 스트림 오류는 `failed`. 스캔은 한 묶음에 `SCAN_COUNT` 를 싣는다.

import type { DbKvScanEvent, KvKeyInfo, RpcClient } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import { KeyScan, SCAN_COUNT } from "./key-scan";
import { KeyTree } from "./key-tree";

const keys = (from: number, n: number): KvKeyInfo[] =>
  Array.from({ length: n }, (_, i) => ({
    key: `k:${from + i}`,
    kind: "string",
    ttlMs: null,
    size: 1,
  }));

/** 항목을 차례로 내는 가짜 스트림. `signal` 이 끊기면 반복을 끝낸다(rpc-client 와 같다). */
function fakeClient(
  events: (DbKvScanEvent | Error)[],
  seen: { params?: unknown; aborted?: boolean },
) {
  return {
    stream(_method: string, params: unknown, options?: { signal?: AbortSignal }) {
      seen.params = params;
      return (async function* () {
        for (const e of events) {
          if (options?.signal?.aborted) {
            seen.aborted = true;
            return;
          }
          await Promise.resolve();
          if (e instanceof Error) throw e;
          yield e;
        }
      })();
    },
  } as unknown as RpcClient;
}

describe("KeyScan", () => {
  it("adds every chunk and ends done with the Host's time", async () => {
    const seen: { params?: unknown } = {};
    const tree = new KeyTree(":", (run) => run());
    const scan = new KeyScan(tree);
    await scan.run(
      fakeClient(
        [
          { kind: "accepted", executionId: "x" },
          { kind: "keys", keys: keys(0, 3) },
          { kind: "keys", keys: keys(3, 2) },
          { kind: "done", keys: 5, elapsedMs: 12 },
        ],
        seen,
      ),
      { sessionId: "s", pattern: "k:*" },
    );
    expect(seen.params).toEqual({ sessionId: "s", pattern: "k:*", count: SCAN_COUNT });
    expect(tree.size).toBe(5);
    expect(scan.state).toEqual({ kind: "done", elapsedMs: 12 });
    expect(scan.firstKeysAt).toBeTypeOf("number");
  });

  it("ends done when the keys exactly fill the tree's key limit", async () => {
    const tree = new KeyTree(":", (run) => run(), 6);
    const scan = new KeyScan(tree);
    await scan.run(
      fakeClient(
        [
          { kind: "keys", keys: keys(0, 3) },
          { kind: "keys", keys: keys(3, 3) },
          { kind: "done", keys: 6, elapsedMs: 1 },
        ],
        {},
      ),
      { sessionId: "s" },
    );
    expect(scan.state).toEqual({ kind: "done", elapsedMs: 1 });
    expect(tree.size).toBe(6);
  });

  it("cuts the stream once keys over the tree's key limit come", async () => {
    const seen: { aborted?: boolean } = {};
    const tree = new KeyTree(":", (run) => run(), 4);
    const scan = new KeyScan(tree);
    await scan.run(
      fakeClient(
        [
          { kind: "keys", keys: keys(0, 3) },
          { kind: "keys", keys: keys(3, 3) },
          { kind: "keys", keys: keys(6, 3) },
          { kind: "done", keys: 9, elapsedMs: 1 },
        ],
        seen,
      ),
      { sessionId: "s" },
    );
    expect(scan.state).toEqual({ kind: "capped" });
    expect(seen.aborted).toBe(true);
    expect(tree.size).toBe(4);
  });

  it("ends failed on a stream error and stopped when stopped", async () => {
    const tree = new KeyTree(":", (run) => run());
    const scan = new KeyScan(tree);
    const boom = new Error("boom");
    await scan.run(fakeClient([{ kind: "keys", keys: keys(0, 1) }, boom], {}), { sessionId: "s" });
    expect(scan.state).toEqual({ kind: "failed", error: boom });

    const running = scan.run(
      fakeClient(
        [
          { kind: "keys", keys: keys(0, 1) },
          { kind: "done", keys: 1, elapsedMs: 1 },
        ],
        {},
      ),
      { sessionId: "s" },
    );
    scan.stop();
    await running;
    expect(scan.state).toEqual({ kind: "stopped" });
  });
});
