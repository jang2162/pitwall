// 키 트리를 채우는 스캔 하나(`db.kv.scan` 스트림, 14-m3-plan M3-22 ⓓ): `accepted` → `keys` … → `done`. 묶음마다 키 트리
// (key-tree.ts)에 붙이고, 끝까지(또는 트리의 보유 상한까지) 읽는다. 멈추면 스트림을 취소한다 — Host 는 credit 을 기다리는 스캔을
// 스트림 취소로 끝낸다.
//
// 스캔은 사람이 [연결] 한 뒤에만 시작한다(문서가 부른다) — 복원된 탭은 스캔하지 않는다(결정 M3-D13).

import type { DbKvScanParams, RpcClient } from "@pitwall/rpc-client";

import type { KeyTree } from "./key-tree";

/** 한 묶음의 키 수(`COUNT`) — Host 의 기본과 같다(14 M3-09 ① 의 1,000키 파이프라인). */
export const SCAN_COUNT = 1_000;

export type ScanState =
  | { kind: "idle" }
  | { kind: "scanning" }
  | { kind: "done"; elapsedMs: number }
  | { kind: "stopped" }
  | { kind: "capped" }
  | { kind: "failed"; error: unknown };

export class KeyScan {
  private stateValue: ScanState = { kind: "idle" };
  private readonly listeners = new Set<() => void>();
  private abort: AbortController | undefined;
  private capped = false;
  /** 첫 `keys` 묶음이 트리에 들어간 때(`performance.now()`) — 측정용. */
  firstKeysAt: number | undefined;

  constructor(readonly tree: KeyTree) {}

  get state(): ScanState {
    return this.stateValue;
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** 스트림을 끝까지 읽는다. 이미 도는 스캔이 있으면 먼저 멈춘다. */
  async run(client: RpcClient, params: Omit<DbKvScanParams, "count">): Promise<void> {
    this.stop();
    const abort = new AbortController();
    this.abort = abort;
    this.capped = false;
    this.set({ kind: "scanning" });
    this.tree.setLoading(true);
    let done: ScanState | undefined;
    try {
      for await (const event of client.stream(
        "db.kv.scan",
        { ...params, count: SCAN_COUNT },
        { signal: abort.signal },
      )) {
        if (event.kind === "keys") {
          const dropped = this.tree.add(event.keys);
          this.firstKeysAt ??= performance.now();
          // 상한을 넘는 키가 왔을 때만 멈춘다 — 키가 꼭 상한만큼이면 스캔은 `done` 으로 끝난다.
          if (dropped > 0) {
            this.capped = true;
            abort.abort();
          }
        } else if (event.kind === "done") {
          done = { kind: "done", elapsedMs: event.elapsedMs };
        }
      }
      if (this.abort !== abort) return;
      this.set(done ?? (this.capped ? { kind: "capped" } : { kind: "stopped" }));
    } catch (error) {
      if (this.abort !== abort) return;
      this.set(
        abort.signal.aborted
          ? this.capped
            ? { kind: "capped" }
            : { kind: "stopped" }
          : { kind: "failed", error },
      );
    } finally {
      if (this.abort === abort) {
        this.abort = undefined;
        this.tree.setLoading(false);
      }
    }
  }

  /** 도는 스캔을 멈춘다(Host 의 스트림도 취소된다). */
  stop() {
    const abort = this.abort;
    if (!abort) return;
    this.abort = undefined;
    abort.abort();
    this.tree.setLoading(false);
    this.set({ kind: "stopped" });
  }

  private set(state: ScanState) {
    this.stateValue = state;
    for (const l of this.listeners) l();
  }
}
