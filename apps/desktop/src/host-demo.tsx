// 웰컴 창에서 UI ↔ LocalHost 왕복(ping · 데모 작업 진행률 · 취소)을 눈으로 확인하는 패널.
// 웰컴 창의 실제 내용(최근 프로젝트 등)은 M0-12 가 채운다. 그때 이 패널과 styles.css 의 .host-demo 를 지운다.

import {
  type DemoTaskUpdate,
  type HostInfo,
  RpcClient,
  type RpcStream,
  type TaskStatus,
} from "@pitwall/rpc-client";
import { useEffect, useRef, useState } from "react";

import { connectHost, type TauriConnection } from "./tauri-transport";

const LOG = "[host-demo]";

// src-tauri/src/main.rs 의 LOCAL_HOST_ID. 웰컴 창에는 프로젝트가 없어 Host 를 직접 고른다.
const LOCAL_HOST = { hostId: "local" };

interface Connected {
  conn: TauriConnection;
  client: RpcClient;
}

export function HostDemo() {
  const [connected, setConnected] = useState<Connected>();
  const [info, setInfo] = useState<HostInfo>();
  const [pong, setPong] = useState<string>("");
  const [progress, setProgress] = useState({ done: 0, total: 0 });
  const [taskState, setTaskState] = useState("idle");
  const stream = useRef<RpcStream<DemoTaskUpdate>>(undefined);
  const pings = useRef(0);

  useEffect(() => {
    let disposed = false;
    let opened: Connected | undefined;
    connectHost(LOCAL_HOST)
      .then(async (conn) => {
        const client = new RpcClient(conn.transport);
        if (disposed) {
          void conn.close();
          return;
        }
        opened = { conn, client };
        setConnected(opened);
        const hostInfo = await client.call("host.info");
        console.info(LOG, "host.info", hostInfo);
        setInfo(hostInfo);
      })
      .catch((e: unknown) => console.error(LOG, "connect failed", e));
    return () => {
      disposed = true;
      opened?.client.close();
      void opened?.conn.close();
    };
  }, []);

  async function ping() {
    if (!connected) return;
    const nonce = `n${++pings.current}`;
    const started = performance.now();
    const result = await connected.client.call("host.ping", { nonce });
    const rtt = (performance.now() - started).toFixed(1);
    console.info(LOG, "host.ping", result, `${rtt}ms`);
    setPong(`${result.nonce} from ${result.hostId} · ${rtt}ms`);
  }

  async function startTask() {
    if (!connected || stream.current) return;
    setTaskState("running");
    setProgress({ done: 0, total: 0 });
    const updates = connected.client.stream("host.demoTask", { steps: 50, intervalMs: 100 });
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

  return (
    <section className="host-demo">
      <h2>Host 연결 확인</h2>
      <p data-testid="host-info">
        {info
          ? `${info.hostId} · ${info.kind} · ${info.os}/${info.arch} · v${info.version}`
          : "연결 중…"}
      </p>
      <div className="row">
        <button type="button" disabled={!connected} onClick={() => void ping()}>
          ping
        </button>
        <output data-testid="pong">{pong}</output>
      </div>
      <div className="row">
        <button
          type="button"
          disabled={!connected || taskState === "running"}
          onClick={() => void startTask()}
        >
          데모 작업 시작
        </button>
        <button
          type="button"
          disabled={taskState !== "running"}
          onClick={() => stream.current?.cancel()}
        >
          취소
        </button>
        <progress max={progress.total || 1} value={progress.done} />
        <output data-testid="task">
          {progress.done}/{progress.total} · {taskState}
        </output>
      </div>
    </section>
  );
}
