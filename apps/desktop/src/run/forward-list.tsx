// 포트 포워딩 목록 · [끄기](18-m7-plan M7-22 ⑤). 상태바 `forwards` 항목(프로젝트 창 — 누르면 목록이 뜬다)과 원격 호스트 시트의 호스트
// 줄(웰컴 창 — 그 호스트의 것만)이 같이 쓴다. 목록은 `remote.forwards.list`, 바뀌면(`remote.forwards` — owner 만의 토픽이라 따로
// 구독한다) 다시 읽는다. **Host 범위 연결로만 부른다** — `remote.*` 는 프로젝트 창의 연결을 거절한다(프로젝트 창은 `useLinkHost`).
//
// 줄마다 이 컴퓨터의 주소 → 그 머신의 주소, 어디서 왔나(설정 · 실행), 열지 못한 설정의 포워딩은 그 까닭(루프백이 아님 · 로컬 포트가
// 쓰임)을 보인다. [끄기] 는 `remote.forward.close` — 실행 탭의 것이면 그 창의 스트림이 끝나고, 설정의 것은 다음에 붙을 때 다시 연다.

import type { RemoteForward } from "@pitwall/rpc-client";
import { useQueryClient } from "@tanstack/react-query";
import { ArrowLeftRight } from "lucide-react";
import { useEffect, useState } from "react";

import { useLinkHost } from "../checkout/checkout-hosts";
import { type HostConnection, useRpcQueryOn } from "../rpc/host";
import { connectEventInvalidation } from "../rpc/invalidation";
import { rpcQueryKey } from "../rpc/queries";
import { useRunStrings } from "./strings";

const LOG = "[forwards]";
const BUTTON =
  "rounded-sm border border-ui-border bg-ui-panel px-1.5 py-0 text-[11px] hover:bg-ui-hover disabled:opacity-50";

/** `host:port`(IPv6 는 대괄호). */
function address(host: string, port: number): string {
  return `${host.includes(":") ? `[${host}]` : host}:${port}`;
}

/** 그 연결의 포워딩 목록 — 바뀌면 다시 읽는다. `name` 이 있으면 그 호스트의 것만. */
export function useForwards(
  connection: Pick<HostConnection, "client" | "key">,
  name?: string,
): readonly RemoteForward[] {
  const queryClient = useQueryClient();
  const { client, key } = connection;
  const list = useRpcQueryOn(
    connection,
    "remote.forwards.list",
    { name: name ?? null },
    { retry: false },
  );
  useEffect(() => {
    if (!client) return;
    const invalidation = connectEventInvalidation(client, queryClient, null, [
      { topic: "remote.forwards", keys: [rpcQueryKey(key, "remote.forwards.list")] },
    ]);
    return () => invalidation.dispose();
  }, [client, key, queryClient]);
  return list.data ?? [];
}

function why(
  f: RemoteForward,
  T: ReturnType<typeof useRunStrings>["forwards"],
): string | undefined {
  if (f.state.state !== "failed") return undefined;
  const { error } = f.state;
  if (error.kind === "notLoopback") return T.notLoopback(f.localHost);
  if (error.kind === "portInUse") return T.portInUse(f.localPort);
  return T.failed(error.message);
}

/** 포워딩 줄들. `name` 이 없으면 모든 호스트(줄마다 호스트 이름이 있다). */
export function ForwardList({
  connection,
  name,
}: {
  connection: Pick<HostConnection, "client" | "key">;
  name?: string;
}) {
  const T = useRunStrings().forwards;
  const forwards = useForwards(connection, name);
  const [error, setError] = useState<string>();
  if (forwards.length === 0)
    return name === undefined ? (
      <p className="px-2 py-1 text-ui-muted-foreground">{T.empty}</p>
    ) : null;
  return (
    <section aria-label={T.title} data-testid="remote-forwards" className="flex flex-col gap-0.5">
      {name !== undefined && <span className="text-[11px] font-semibold">{T.title}</span>}
      <ul className="flex flex-col gap-0.5">
        {forwards.map((f) => {
          const reason = why(f, T);
          return (
            <li
              key={f.id}
              data-testid="remote-forward"
              data-forward-id={f.id}
              data-state={f.state.state}
              className="flex items-center gap-2 text-[11px]"
            >
              <span className="min-w-0 flex-1 truncate font-mono">
                {T.row(
                  address(f.localHost, f.localPort),
                  address(f.remoteHost, f.remotePort),
                  f.name,
                )}
              </span>
              <span className="flex-none rounded-sm border border-ui-border px-1">
                {T.origin[f.origin]}
              </span>
              {reason && (
                <span data-testid="remote-forward-why" className="text-ui-accent">
                  {reason}
                </span>
              )}
              <button
                type="button"
                data-testid="remote-forward-off"
                className={BUTTON}
                disabled={!connection.client}
                onClick={() =>
                  void connection.client
                    ?.call("remote.forward.close", { id: f.id })
                    .catch((e: unknown) => {
                      console.warn(LOG, "could not turn off the forward", f.id, e);
                      setError(e instanceof Error ? e.message : String(e));
                    })
                }
              >
                {T.off}
              </button>
            </li>
          );
        })}
      </ul>
      {error && (
        <span role="alert" className="text-[11px] text-ui-accent">
          {error}
        </span>
      )}
    </section>
  );
}

/** 상태바 항목: 이 앱의 포트 포워딩 수. 0 이면 그리지 않는다. 누르면 목록 · [끄기]. */
export function ForwardsStatusItem() {
  const T = useRunStrings().forwards;
  const link = useLinkHost();
  const forwards = useForwards(link);
  const [open, setOpen] = useState(false);
  if (forwards.length === 0) return null;
  return (
    <span className="relative flex items-center">
      <button
        type="button"
        data-testid="status-forwards"
        aria-expanded={open}
        title={T.statusHint(forwards.length)}
        onClick={() => setOpen((v) => !v)}
        className="flex items-center gap-1 rounded-sm px-1 hover:bg-ui-hover"
      >
        <ArrowLeftRight size={11} aria-hidden />
        {T.status(forwards.length)}
      </button>
      {open && (
        <div
          role="dialog"
          aria-label={T.title}
          className="absolute right-0 bottom-full z-50 mb-1 w-[420px] rounded-sm border border-ui-border bg-ui-panel p-2 shadow-md"
        >
          <ForwardList connection={link} />
        </div>
      )}
    </span>
  );
}
