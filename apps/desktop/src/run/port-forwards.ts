// 실행 탭 포트의 「로컬로 포워딩」(18-m7-plan M7-22 ① ④). 다른 머신 실행의 포트를 누르면 창의 **Host 범위 연결**(`useLinkHost` —
// `remote.*` 는 Host 범위만 받는다)로 `remote.forward.open` 스트림을 연다. 스트림이 열린 동안 Host 가 이 컴퓨터의 루프백
// (`127.0.0.1`)에서 듣고 그 머신의 `localhost:<포트>` 로 잇는다. 원격 쪽 호스트는 고를 수 없다(Host 가 늘 `localhost`).
//
// - **그 실행이 사는 동안**: 실행 탭에서 그 실행이 끝나거나(다시 실행으로 다른 실행이 되거나) 탭이 닫히면 스트림을 닫는다 — Host 가
//   포워딩을 닫는다. 창이 닫히면 연결과 함께 스트림이 끝난다.
// - 로컬 포트가 이미 쓰이면(`conflict` · `portInUse`) 그 사실과 그 번호를 보이고 사람이 다른 포트를 고른다(빈 칸이면 OS 가 고른다).
// - 상태바 · 원격 호스트 시트의 [끄기](`remote.forward.close`)나 링크 끊김으로 Host 가 닫으면 스트림이 끝나 항목이 빠진다.
// - 창마다 웹뷰가 따로라 모듈 하나가 창 하나다(`windowRuns` 와 같은 꼴).

import type { RemoteForward } from "@pitwall/rpc-client";
import { RpcCallError } from "@pitwall/rpc-client";
import { createStore } from "zustand/vanilla";

import { isLive, type RunClient, type RunTab, windowRuns } from "./run-tabs";

const LOG = "[run-forward]";
const CONFLICT = -32004;

export type RunForwardState =
  | { kind: "opening" }
  | { kind: "on"; forward: RemoteForward }
  /** 이 컴퓨터에서 그 로컬 포트를 이미 누가 쓴다 — 다른 포트를 고른다. */
  | { kind: "inUse"; port: number }
  | { kind: "failed"; message: string };

export interface RunForward {
  key: string;
  machine: string;
  runId: string;
  remotePort: number;
  state: RunForwardState;
}

export interface PortForwardsState {
  forwards: Readonly<Record<string, RunForward>>;
}

/** 실행 하나의 포트 하나. */
export function forwardKey(machine: string, runId: string, remotePort: number): string {
  return `${machine}\n${runId}\n${remotePort}`;
}

export class PortForwards {
  readonly store = createStore<PortForwardsState>()(() => ({ forwards: {} }));
  private readonly aborts = new Map<string, AbortController>();

  /**
   * 그 실행의 포트를 로컬로 포워딩한다. `link` 는 창의 Host 범위 연결, `name` 은 그 머신의 원격 호스트 이름. `localPort` 가 없으면
   * 같은 번호, `0` 이면 OS 가 고른다. 이미 열렸거나 여는 중이면 아무것도 하지 않는다.
   */
  open(
    link: Pick<RunClient, "stream">,
    target: { name: string; machine: string; runId: string; remotePort: number },
    localPort?: number,
  ): void {
    const key = forwardKey(target.machine, target.runId, target.remotePort);
    const now = this.store.getState().forwards[key];
    if (now && (now.state.kind === "opening" || now.state.kind === "on")) return;
    this.aborts.get(key)?.abort();
    const abort = new AbortController();
    this.aborts.set(key, abort);
    const base = {
      key,
      machine: target.machine,
      runId: target.runId,
      remotePort: target.remotePort,
    };
    this.set(key, { ...base, state: { kind: "opening" } });
    void (async () => {
      try {
        for await (const forward of link.stream(
          "remote.forward.open",
          { name: target.name, remotePort: target.remotePort, localPort: localPort ?? null },
          { signal: abort.signal },
        )) {
          if (abort.signal.aborted) return;
          this.set(key, { ...base, state: { kind: "on", forward } });
        }
        // Host 가 닫았다([끄기] · 링크 끊김).
        if (this.aborts.get(key) === abort) this.forget(key);
      } catch (e) {
        if (abort.signal.aborted || this.aborts.get(key) !== abort) return;
        this.aborts.delete(key);
        const data =
          e instanceof RpcCallError ? (e.data as { reason?: unknown; port?: unknown }) : null;
        if (e instanceof RpcCallError && e.code === CONFLICT && data?.reason === "portInUse")
          this.set(key, { ...base, state: { kind: "inUse", port: Number(data.port) } });
        else {
          console.warn(LOG, "could not forward", target.remotePort, e);
          this.set(key, {
            ...base,
            state: { kind: "failed", message: e instanceof Error ? e.message : String(e) },
          });
        }
      }
    })();
  }

  /** 포워딩을 멈춘다(스트림을 닫으면 Host 가 닫는다) · 오류 표시를 걷는다. */
  stop(key: string): void {
    this.aborts.get(key)?.abort();
    this.forget(key);
  }

  /** 창의 실행 탭을 따라간다: 포워딩한 실행이 더는 돌지 않으면(끝남 · 다른 실행 · 탭 닫힘) 그 포워딩을 멈춘다. */
  follow(tabs: readonly Pick<RunTab, "machine" | "run">[]): void {
    const live = new Set(
      tabs.filter((t) => isLive(t.run)).map((t) => `${t.machine}\n${t.run.runId}`),
    );
    for (const f of Object.values(this.store.getState().forwards))
      if (!live.has(`${f.machine}\n${f.runId}`)) this.stop(f.key);
  }

  /** 모두 멈춘다(시험). */
  disposeAll(): void {
    for (const abort of this.aborts.values()) abort.abort();
    this.aborts.clear();
    this.store.setState({ forwards: {} });
  }

  private set(key: string, forward: RunForward) {
    this.store.setState((s) => ({ forwards: { ...s.forwards, [key]: forward } }));
  }

  private forget(key: string) {
    this.aborts.delete(key);
    if (!(key in this.store.getState().forwards)) return;
    this.store.setState((s) => {
      const { [key]: _gone, ...rest } = s.forwards;
      return { forwards: rest };
    });
  }
}

let current: PortForwards | undefined;

/** 이 창의 실행 탭 포워딩. 처음 부를 때 창의 실행 탭을 따라가기 시작한다. */
export function windowForwards(): PortForwards {
  if (!current) {
    const forwards = new PortForwards();
    current = forwards;
    const runs = windowRuns().store;
    runs.subscribe((s) => forwards.follow(s.tabs));
  }
  return current;
}
