// 웰컴 창에서 UI ↔ LocalHost 왕복(ping · 데모 작업 진행률 · 취소)을 눈으로 확인하는 패널.
// 웰컴 창의 실제 내용(최근 프로젝트 등)은 M0-12 가 채운다. 그때 이 패널을 지운다.

import type { DemoTaskUpdate, RpcStream, TaskStatus } from "@pitwall/rpc-client";
import { useEffect, useRef, useState } from "react";

import { useHost, useRpcQuery } from "./rpc/host";

const LOG = "[host-demo]";

const BUTTON =
  "rounded-sm border border-ui-border bg-ui-panel px-2 py-0.5 hover:bg-ui-hover disabled:opacity-50";

export function HostDemo() {
  const { client } = useHost();
  const info = useRpcQuery("host.info", null);
  const [pong, setPong] = useState<string>("");
  const [progress, setProgress] = useState({ done: 0, total: 0 });
  const [taskState, setTaskState] = useState("idle");
  const stream = useRef<RpcStream<DemoTaskUpdate>>(undefined);
  const pings = useRef(0);

  useEffect(() => {
    if (info.data) console.info(LOG, "host.info", info.data);
    if (info.error) console.error(LOG, "host.info failed", info.error);
  }, [info.data, info.error]);

  async function ping() {
    if (!client) return;
    const nonce = `n${++pings.current}`;
    const started = performance.now();
    const result = await client.call("host.ping", { nonce });
    const rtt = (performance.now() - started).toFixed(1);
    console.info(LOG, "host.ping", result, `${rtt}ms`);
    setPong(`${result.nonce} from ${result.hostId} · ${rtt}ms`);
  }

  async function startTask() {
    if (!client || stream.current) return;
    setTaskState("running");
    setProgress({ done: 0, total: 0 });
    const updates = client.stream("host.demoTask", { steps: 50, intervalMs: 100 });
    stream.current = updates;
    let outcome: TaskStatus | undefined;
    try {
      for await (const update of updates) {
        if (update.kind === "progress") {
          setProgress({ done: update.progress.done, total: update.progress.total ?? 0 });
        } else {
          outcome = update.outcome;
        }
      }
      // 취소하면 종료 청크 없이 반복이 끝난다.
      console.info(LOG, "task ended", outcome ?? "cancelled");
      setTaskState(outcome?.status ?? "cancelled");
    } catch (e) {
      console.error(LOG, "task stream failed", e);
      setTaskState("failed");
    } finally {
      stream.current = undefined;
    }
  }

  const hostInfo = info.data;
  return (
    <section className="px-6 py-4">
      <h2 className="mb-2 text-sm font-semibold">Host 연결 확인</h2>
      <p data-testid="host-info">
        {hostInfo
          ? `${hostInfo.hostId} · ${hostInfo.kind} · ${hostInfo.os}/${hostInfo.arch} · v${hostInfo.version}`
          : "연결 중…"}
      </p>
      <div className="my-2 flex items-center gap-2">
        <button type="button" className={BUTTON} disabled={!client} onClick={() => void ping()}>
          ping
        </button>
        <output data-testid="pong">{pong}</output>
      </div>
      <div className="my-2 flex items-center gap-2">
        <button
          type="button"
          className={BUTTON}
          disabled={!client || taskState === "running"}
          onClick={() => void startTask()}
        >
          데모 작업 시작
        </button>
        <button
          type="button"
          className={BUTTON}
          disabled={taskState !== "running"}
          onClick={() => stream.current?.cancel()}
        >
          취소
        </button>
        <progress className="accent-ui-accent" max={progress.total || 1} value={progress.done} />
        <output data-testid="task">
          {progress.done}/{progress.total} · {taskState}
        </output>
      </div>
    </section>
  );
}
