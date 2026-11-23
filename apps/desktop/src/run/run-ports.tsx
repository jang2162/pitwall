// 실행이 듣는 포트의 칩(17-m6-plan M6-08 — 「실행」 툴 윈도우의 탭 머리 · 상태바 `run` 항목). 포트는 Host 가 실행의 프로세스
// 트리에서 읽어 `RunInfo.ports` · 이벤트 `run.ports` 로 낸다(run-tabs.ts 가 받아 탭의 `run` 을 바꾼다). 칩을 누르면 바깥 브라우저로
// `http://localhost:<port>` 를 연다 — `0.0.0.0` · `::` 에 들어도 `localhost` 이고, IPv6 주소에만 들으면 `http://[::1]:<port>`
// (`localhost` 가 127.0.0.1 로 풀리면 닿지 않는다). `0.0.0.0` · `::` 에 듣는 포트는 다른 기기에서도 닿으므로 「모든 주소」 표지를
// 함께 보인다. 끝난 실행 · 포트가 없으면 `null` 이다 — 그때는 자리도 차지하지 않는다.
//
// **다른 머신의 실행**(18-m7-plan M7-14 ③): 그 포트는 그 머신의 포트다 — `localhost` 로 열면 이 컴퓨터의 같은 번호(다른 프로그램)가
// 열린다. 그래서 다른 머신의 칩은 브라우저를 열지 않고 「<머신> 의 포트」 라고 글로 보이며, 옆에 「로컬로 포워딩」 단추 자리를 둔다
// (M7-22 가 채운다 — 지금은 꺼짐). 상태바는 머신마다 따로 모은다(두 머신의 같은 번호를 하나로 합치지 않는다).

import type { RunInfo, RunPort } from "@pitwall/rpc-client";
import { ArrowDownToLine } from "lucide-react";
import type { ReactNode } from "react";

import { LOCAL_MACHINE } from "../checkout/checkout";
import { useMachineName } from "../checkout/machine-mark";
import { isLive, type RunTab, windowRuns } from "./run-tabs";
import { useRunStrings } from "./strings";

/** 칩이 여는 주소. */
export function portUrl(port: RunPort): string {
  const ipv6Only = port.addresses.length > 0 && port.addresses.every((a) => a.includes(":"));
  return `http://${ipv6Only ? "[::1]" : "localhost"}:${port.port}`;
}

function PortChip({
  port,
  testId,
  machine = LOCAL_MACHINE,
}: {
  port: RunPort;
  testId: string;
  machine?: string;
}) {
  if (machine !== LOCAL_MACHINE)
    return <RemotePortChip port={port} testId={testId} machine={machine} />;
  return <LocalPortChip port={port} testId={testId} />;
}

/** 다른 머신의 포트: 열지 않는다. 「로컬로 포워딩」 자리(M7-22). */
function RemotePortChip({
  port,
  testId,
  machine,
}: {
  port: RunPort;
  testId: string;
  machine: string;
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
      <button
        type="button"
        data-testid="run-port-forward"
        disabled
        title={T.forwardSoon}
        aria-label={`${T.forward} — ${title}`}
        onClick={(e) => e.stopPropagation()}
        className="flex items-center rounded-sm px-0.5 text-ui-muted-foreground disabled:opacity-50"
      >
        <ArrowDownToLine size={10} aria-hidden />
      </button>
    </span>
  );
}

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
        <PortChip key={port.port} port={port} testId="run-tab-port" machine={machine} />
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
  const chips = machines.flatMap((machine) =>
    mergedPorts(byMachine.get(machine)!).map((port) => (
      <PortChip
        key={`${machine}:${port.port}`}
        port={port}
        testId="status-run-port"
        machine={machine}
      />
    )),
  );
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
