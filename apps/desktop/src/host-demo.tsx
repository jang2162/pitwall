// 임시 화면: 웰컴 창에서 UI ↔ LocalHost 왕복(ping · 데모 작업 진행률 · 취소)을 눈으로 확인한다(M0-07).
// M0-06 의 rpc-client 로 교체할 때 rpc-temp.ts 와 함께 지운다. 웰컴 창의 실제 내용은 M0-12 가 채운다.

import { useEffect, useRef, useState } from "react";

import { type StreamHandle, type TempConnection, connectLocalHost } from "./rpc-temp";

interface HostInfo {
  hostId: string;
  kind: string;
  version: string;
  os: string;
  arch: string;
}

type DemoUpdate =
  | {
      kind: "progress";
      taskId: number;
      progress: { done: number; total?: number; message?: string };
    }
  | { kind: "finished"; taskId: number; status: string; error?: string };

const LOG = "[host-demo]";

export function HostDemo() {
  const [conn, setConn] = useState<TempConnection>();
  const [info, setInfo] = useState<HostInfo>();
  const [pong, setPong] = useState<string>("");
  const [progress, setProgress] = useState({ done: 0, total: 0 });
  const [taskState, setTaskState] = useState("idle");
  const stream = useRef<StreamHandle>(undefined);
  const pings = useRef(0);

  useEffect(() => {
    let closed = false;
    let opened: TempConnection | undefined;
    connectLocalHost()
      .then(async (c) => {
        if (closed) return void c.close();
        opened = c;
        setConn(c);
        const hostInfo = await c.client.request<HostInfo>("host.info");
        console.info(LOG, "host.info", hostInfo);
        setInfo(hostInfo);
      })
      .catch((e: unknown) => console.error(LOG, "connect failed", e));
    return () => {
      closed = true;
      void opened?.close();
    };
  }, []);

  async function ping() {
    if (!conn) return;
    const nonce = `n${++pings.current}`;
    const started = performance.now();
    const result = await conn.client.request<{ nonce: string; hostId: string }>("host.ping", {
      nonce,
    });
    const rtt = (performance.now() - started).toFixed(1);
    console.info(LOG, "host.ping", result, `${rtt}ms`);
    setPong(`${result.nonce} from ${result.hostId} · ${rtt}ms`);
  }

  async function startTask() {
    if (!conn || stream.current) return;
    setTaskState("running");
    setProgress({ done: 0, total: 0 });
    const handle = await conn.client.stream(
      "host.demoTask",
      { steps: 50, intervalMs: 100 },
      (item) => {
        const update = item as DemoUpdate;
        if (update.kind === "progress") {
          setProgress({ done: update.progress.done, total: update.progress.total ?? 0 });
        } else {
          console.info(LOG, "task finished", update);
          setTaskState(update.status);
        }
      },
    );
    stream.current = handle;
    const end = await handle.done;
    console.info(LOG, "stream end", end);
    stream.current = undefined;
    // 취소하면 작업 종료 청크 없이 stream.end(cancelled) 로 끝난다.
    if (end.status !== "completed") setTaskState(end.status);
  }

  return (
    <section className="host-demo">
      <h2>Host 연결 확인 (임시)</h2>
      <p data-testid="host-info">
        {info
          ? `${info.hostId} · ${info.kind} · ${info.os}/${info.arch} · v${info.version}`
          : "연결 중…"}
      </p>
      <div className="row">
        <button type="button" disabled={!conn} onClick={() => void ping()}>
          ping
        </button>
        <output data-testid="pong">{pong}</output>
      </div>
      <div className="row">
        <button
          type="button"
          disabled={!conn || taskState === "running"}
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
