// 실행이 듣는 포트의 칩(17-m6-plan M6-08 — 「실행」 툴 윈도우의 탭 머리 · 상태바 `run` 항목). 포트는 Host 가 실행의 프로세스
// 트리에서 읽어 `RunInfo.ports` · 이벤트 `run.ports` 로 낸다(run-tabs.ts 가 받아 탭의 `run` 을 바꾼다). 칩을 누르면 바깥 브라우저로
// `http://localhost:<port>` 를 연다 — `0.0.0.0` · `::` 에 들어도 `localhost` 이고, IPv6 주소에만 들으면 `http://[::1]:<port>`
// (`localhost` 가 127.0.0.1 로 풀리면 닿지 않는다). `0.0.0.0` · `::` 에 듣는 포트는 다른 기기에서도 닿으므로 「모든 주소」 표지를
// 함께 보인다. 끝난 실행 · 포트가 없으면 `null` 이다 — 그때는 자리도 차지하지 않는다.

import type { RunInfo, RunPort } from "@pitwall/rpc-client";
import type { ReactNode } from "react";

import { isLive, windowRuns } from "./run-tabs";
import { useRunStrings } from "./strings";

/** 칩이 여는 주소. */
export function portUrl(port: RunPort): string {
  const ipv6Only = port.addresses.length > 0 && port.addresses.every((a) => a.includes(":"));
  return `http://${ipv6Only ? "[::1]" : "localhost"}:${port.port}`;
}

function PortChip({ port, testId }: { port: RunPort; testId: string }) {
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

/** 탭 머리의 포트(예: `:5173`). */
export function RunTabPorts({ run }: { run: RunInfo }): ReactNode {
  if (!isLive(run) || run.ports.length === 0) return null;
  return (
    <span className="flex items-center gap-0.5">
      {run.ports.map((port) => (
        <PortChip key={port.port} port={port} testId="run-tab-port" />
      ))}
    </span>
  );
}

/** 상태바 `run` 항목 옆의 포트 — 도는 실행들의 포트를 포트 순으로 한 번씩. */
export function RunStatusPorts({ runs }: { runs: readonly RunInfo[] }): ReactNode {
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
  if (byPort.size === 0) return null;
  return (
    <span className="flex items-center gap-0.5">
      {[...byPort.values()]
        .sort((a, b) => a.port - b.port)
        .map((port) => (
          <PortChip key={port.port} port={port} testId="status-run-port" />
        ))}
    </span>
  );
}
