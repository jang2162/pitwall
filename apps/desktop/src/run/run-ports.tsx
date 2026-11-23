// 실행이 듣는 포트의 칩(17-m6-plan M6-08 — 「실행」 툴 윈도우의 탭 머리 · 상태바 `run` 항목). 포트는 Host 가 실행의 프로세스
// 트리에서 읽어 `RunInfo.ports` · 이벤트 `run.ports` 로 낸다(run-tabs.ts 가 받아 탭의 `run` 을 바꾼다). 칩을 누르면 바깥 브라우저로
// `http://localhost:<port>` 를 연다 — `0.0.0.0` · `::` 에 들어도 `localhost` 이고, IPv6 주소에만 들으면 `http://[::1]:<port>`
// (`localhost` 가 127.0.0.1 로 풀리면 닿지 않는다). `0.0.0.0` · `::` 에 듣는 포트는 다른 기기에서도 닿으므로 「모든 주소」 표지를
// 함께 보인다. 끝난 실행 · 포트가 없으면 `null` 이다 — 그때는 자리도 차지하지 않는다.
//
// **다른 머신의 실행**(18-m7-plan M7-14 ③): 그 포트는 그 머신의 포트다 — `localhost` 로 열면 이 컴퓨터의 같은 번호(다른 프로그램)가
// 열린다. 그래서 다른 머신의 칩은 브라우저를 열지 않고 「<머신> 의 포트」 라고 글로 보이며, 옆에 「로컬로 포워딩」 단추를 둔다
// (M7-22 — port-forwards.ts). 포워딩하면 칩 옆에 이 컴퓨터의 루프백 주소가 생기고 그것을 누르면 브라우저로 연다. 로컬 포트가 이미
// 쓰이면 그 사실과 다른 포트를 고르는 칸을 보인다. 상태바는 머신마다 따로 모은다(두 머신의 같은 번호를 하나로 합치지 않는다).

import type { RunInfo, RunPort } from "@pitwall/rpc-client";
import { ArrowDownToLine, X } from "lucide-react";
import { type ReactNode, useState } from "react";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import { LOCAL_MACHINE } from "../checkout/checkout";
import { useCheckoutHosts, useLinkHost } from "../checkout/checkout-hosts";
import { useMachineName } from "../checkout/machine-mark";
import { forwardKey, windowForwards } from "./port-forwards";
import { isLive, type RunTab, windowRuns } from "./run-tabs";
import { useRunStrings } from "./strings";

const EMPTY_NAMES: ReadonlyMap<string, string> = new Map();

/** 칩이 여는 주소. */
export function portUrl(port: RunPort): string {
  const ipv6Only = port.addresses.length > 0 && port.addresses.every((a) => a.includes(":"));
  return `http://${ipv6Only ? "[::1]" : "localhost"}:${port.port}`;
}

function PortChip({
  port,
  testId,
  machine = LOCAL_MACHINE,
  runId,
}: {
  port: RunPort;
  testId: string;
  machine?: string;
  /** 다른 머신 실행이면 그 실행 — 포워딩은 그 실행이 사는 동안이다. 없으면 포워딩 단추가 없다. */
  runId?: string;
}) {
  if (machine !== LOCAL_MACHINE)
    return <RemotePortChip port={port} testId={testId} machine={machine} runId={runId} />;
  return <LocalPortChip port={port} testId={testId} />;
}

/** 다른 머신의 포트: 이 컴퓨터로 열지 않는다. 「로컬로 포워딩」(M7-22). */
function RemotePortChip({
  port,
  testId,
  machine,
  runId,
}: {
  port: RunPort;
  testId: string;
  machine: string;
  runId?: string;
}) {
  const T = useRunStrings().port;
  const name = useMachineName()(machine);
  const title = T.remote(port.port, name);
  return (
    <span className="flex items-center gap-0.5">
      <span
        data-testid={testId}
        data-port={port.port}
        data-machine={machine}
        title={title}
        aria-label={title}
        className="rounded-sm px-0.5 font-mono text-[10px] text-ui-muted-foreground"
      >
        {T.label(port.port)}
      </span>
      {runId !== undefined && (
        <ForwardControl port={port.port} machine={machine} runId={runId} title={title} />
      )}
    </span>
  );
}

/** 포워딩 단추 · 상태: 끔 → [로컬로 포워딩], 여는 중, 켬 → 이 컴퓨터 주소(누르면 연다) · [멈추기], 포트가 쓰임 → 다른 포트 칸. */
function ForwardControl({
  port,
  machine,
  runId,
  title,
}: {
  port: number;
  machine: string;
  runId: string;
  title: string;
}) {
  const T = useRunStrings().port;
  const forwards = windowForwards();
  const key = forwardKey(machine, runId, port);
  const entry = useStore(forwards.store, (s) => s.forwards[key]);
  const link = useLinkHost().client;
  const hosts = useCheckoutHosts();
  const names = useStore(
    hosts?.store ?? FALLBACK_NAMES,
    (s: { directory: { hostNames: ReadonlyMap<string, string> } }) => s.directory.hostNames,
  );
  const hostName = names.get(machine);
  const [other, setOther] = useState("");
  const open = (localPort?: number) => {
    if (!link || hostName === undefined) return;
    forwards.open(link, { name: hostName, machine, runId, remotePort: port }, localPort);
  };
  const state = entry?.state;
  if (state?.kind === "on") {
    const url = `http://localhost:${state.forward.localPort}`;
    return (
      <span className="flex items-center gap-0.5">
        <button
          type="button"
          data-testid="run-port-forwarded"
          data-local-port={state.forward.localPort}
          title={T.forwarded(url)}
          aria-label={`${T.forwarded(url)} — ${title}`}
          onClick={(e) => {
            e.stopPropagation();
            windowRuns().openUrl(url);
          }}
          className="rounded-sm px-0.5 font-mono text-[10px] text-ui-foreground hover:bg-ui-hover"
        >
          {`→ ${T.label(state.forward.localPort)}`}
        </button>
        <button
          type="button"
          data-testid="run-port-forward-stop"
          title={T.forwardStop}
          aria-label={T.forwardStop}
          onClick={(e) => {
            e.stopPropagation();
            forwards.stop(key);
          }}
          className="flex items-center rounded-sm px-0.5 text-ui-muted-foreground hover:bg-ui-hover"
        >
          <X size={10} aria-hidden />
        </button>
      </span>
    );
  }
  if (state?.kind === "inUse")
    return (
      <span className="flex items-center gap-0.5" onClick={(e) => e.stopPropagation()}>
        <span
          role="alert"
          data-testid="run-port-forward-in-use"
          className="text-[10px] text-ui-accent"
        >
          {T.forwardInUse(state.port)}
        </span>
        <input
          type="number"
          min={1}
          max={65535}
          data-testid="run-port-forward-port"
          aria-label={T.forwardOther}
          title={T.forwardOtherHint}
          placeholder={T.forwardOtherHint}
          value={other}
          onChange={(e) => setOther(e.target.value)}
          className="w-[64px] rounded-sm border border-ui-border bg-ui-background px-0.5 text-[10px]"
        />
        <button
          type="button"
          data-testid="run-port-forward-other"
          onClick={() => {
            const chosen = Number.parseInt(other, 10);
            open(Number.isInteger(chosen) && chosen > 0 && chosen < 65536 ? chosen : 0);
          }}
          className="rounded-sm border border-ui-border px-0.5 text-[10px] hover:bg-ui-hover"
        >
          {T.forwardRetry}
        </button>
      </span>
    );
  const failed = state?.kind === "failed" ? T.forwardFailed(state.message) : undefined;
  return (
    <button
      type="button"
      data-testid="run-port-forward"
      disabled={!link || hostName === undefined || state?.kind === "opening"}
      title={failed ?? (state?.kind === "opening" ? T.forwardOpening : T.forward)}
      aria-label={`${T.forward} — ${title}`}
      data-failed={failed !== undefined || undefined}
      onClick={(e) => {
        e.stopPropagation();
        open();
      }}
      className="flex items-center rounded-sm px-0.5 text-ui-muted-foreground hover:bg-ui-hover disabled:opacity-50 data-[failed]:text-ui-accent"
    >
      <ArrowDownToLine size={10} aria-hidden />
    </button>
  );
}

const FALLBACK_NAMES = createStore<{ directory: { hostNames: ReadonlyMap<string, string> } }>()(
  () => ({
    directory: { hostNames: EMPTY_NAMES },
  }),
);

function LocalPortChip({ port, testId }: { port: RunPort; testId: string }) {
  const T = useRunStrings().port;
  const url = portUrl(port);
  const open = T.open(url);
  const title = port.allAddresses ? `${open} · ${T.allAddresses}` : open;
  return (
    <button
      type="button"
      data-testid={testId}
      data-port={port.port}
      data-all-addresses={port.allAddresses}
      title={title}
      aria-label={title}
      onClick={(e) => {
        e.stopPropagation();
        windowRuns().openUrl(url);
      }}
      className="flex items-center gap-0.5 rounded-sm px-0.5 font-mono text-[10px] text-ui-muted-foreground hover:bg-ui-hover hover:text-ui-foreground"
    >
      {T.label(port.port)}
      {port.allAddresses && (
        <span
          data-testid="run-port-all-addresses"
          className="rounded-sm border border-ui-border px-0.5 font-sans text-[9px]"
        >
          {T.allAddresses}
        </span>
      )}
    </button>
  );
}

/** 탭 머리의 포트(예: `:5173`). `machine` 은 그 실행의 머신. */
export function RunTabPorts({
  run,
  machine = LOCAL_MACHINE,
}: {
  run: RunInfo;
  machine?: string;
}): ReactNode {
  if (!isLive(run) || run.ports.length === 0) return null;
  return (
    <span className="flex items-center gap-0.5">
      {run.ports.map((port) => (
        <PortChip
          key={port.port}
          port={port}
          testId="run-tab-port"
          machine={machine}
          runId={run.runId}
        />
      ))}
    </span>
  );
}

/** 상태바 `run` 항목 옆의 포트 — 도는 실행들의 포트를 머신마다 포트 순으로 한 번씩(이 머신 먼저). */
export function RunStatusPorts({
  tabs,
}: {
  tabs: readonly Pick<RunTab, "run" | "machine">[];
}): ReactNode {
  const byMachine = new Map<string, RunInfo[]>();
  for (const tab of tabs)
    byMachine.set(tab.machine, [...(byMachine.get(tab.machine) ?? []), tab.run]);
  const machines = [...byMachine.keys()].sort((a, b) =>
    a === LOCAL_MACHINE ? -1 : b === LOCAL_MACHINE ? 1 : a.localeCompare(b),
  );
  const chips = machines.flatMap((machine) => {
    const runs = byMachine.get(machine)!;
    return mergedPorts(runs).map((port) => (
      <PortChip
        key={`${machine}:${port.port}`}
        port={port}
        testId="status-run-port"
        machine={machine}
        // 그 포트에 듣는 첫 실행 — 포워딩은 그 실행이 사는 동안이다(탭 머리와 같은 열쇠).
        runId={runs.find((r) => isLive(r) && r.ports.some((p) => p.port === port.port))?.runId}
      />
    ));
  });
  if (chips.length === 0) return null;
  return <span className="flex items-center gap-0.5">{chips}</span>;
}

/** 한 머신의 도는 실행들의 포트를 포트 순으로 한 번씩. */
function mergedPorts(runs: readonly RunInfo[]): RunPort[] {
  const byPort = new Map<number, RunPort>();
  for (const run of runs) {
    if (!isLive(run)) continue;
    for (const port of run.ports) {
      const seen = byPort.get(port.port);
      byPort.set(
        port.port,
        seen
          ? {
              port: port.port,
              allAddresses: seen.allAddresses || port.allAddresses,
              addresses: [...new Set([...seen.addresses, ...port.addresses])].sort(),
            }
          : port,
      );
    }
  }
  return [...byPort.values()].sort((a, b) => a.port - b.port);
}
