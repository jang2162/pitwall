// 웰컴 창의 「원격 호스트」 탭(10-ui.md S8, 18-m7-plan M7-08 ⑤). 목록은 `remote.hosts.list`(설정의 `[[remote_hosts]]` + `~/.ssh/config`
// 의 Host 이름), 상태가 바뀌면(`remote.status`) 다시 읽는다. [연결] 은 `remote.connect` 스트림을 이 창이 여는 동안 그 호스트를
// 「쓴다」 — 창이 닫히거나 탭을 떠나면 놓고, 쓰는 것이 모두 없어지면 Host 가 링크를 닫는다(연결은 쓰는 머신만).
//
// - 진단 칸: SSH → 설치 → 데몬 → 핸드셰이크 단계와 실패한 단계, 「연결 로그」(`remote.log` — ssh stderr 끝부분, askpass 답은 없다).
//   로그 · 실패 글은 원격이 준 글이 섞일 수 있어 그대로(번역 없이) 글자로만 보인다 — Host 가 제어 · 양방향 글자를 뺐다.
// - 머신이 바뀌었으면(`machineChanged`) 두 id 를 보이고 [이 머신을 새로 믿기] 만 둔다(메인 판정 M7-P6).
// - 붙은 뒤에는 그 링크에서 본 원격 Relay(`remote.relays` — 붙을 때마다 조사 · 정리가 한 번 돈다, 18-m7-plan M7-07b)를 보인다:
//   떠 있는 옛 판 데몬마다 남은 실행 · 터미널 수와 [옛 판 끝내기]. 끝내기는 **보인 값 그대로**(pid · 실행 · 터미널 · 붙은 링크 수와
//   조사한 머신 id)를 `remote.stopOld` 에 싣는다 — 그사이 바뀌었으면 Host 가 끝내지 않고 새 수를 보인다. 다른 Desktop 이 그 옛 판에
//   붙어 있으면 그 수와 「그 연결도 끊긴다」 를 보이고 단추 글도 다르다. 원격의 `pitwall` 명령 링크(`~/.local/bin/pitwall`)는 사람이
//   고를 때만 둔다(`remote.linkCommand` — 메인 판정 M7-P12).
// - ssh 의 질문(호스트 키 확인 · 암호)은 `RemotePrompts` 가 `remote.prompts` 로 듣고 git 과 같은 askpass 다이얼로그로 묻는다 —
//   호스트 키 질문은 [취소](처음 포커스) · [예](`yes`)다(git-prompts.tsx).

import type {
  GitPromptAnswer,
  RemoteHostInfo,
  RemoteOldRelay,
  RemoteStage,
  RemoteStatus,
} from "@pitwall/rpc-client";
import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";

import { type AskpassPrompt, AskpassDialog } from "../git/git-prompts";
import { useI18n } from "../i18n/i18n-provider";
import { useHost, useRpcQuery } from "../rpc/host";
import { connectEventInvalidation } from "../rpc/invalidation";
import { rpcQueryKey } from "../rpc/queries";
import { type WelcomeStrings, useWelcomeStrings } from "./strings";

const LOG = "[remote]";
const BUTTON =
  "rounded-sm border border-ui-border bg-ui-panel px-2 py-0.5 hover:bg-ui-hover disabled:opacity-50";

export const STAGES: readonly RemoteStage[] = ["ssh", "install", "daemon", "handshake"];

export type StageMark = "done" | "active" | "failed" | "pending";

/** 상태에서 단계마다의 표시: 지금 단계 앞은 끝남, 실패한 단계는 실패. */
export function stageMarks(status: RemoteStatus): Record<RemoteStage, StageMark> {
  const marks = (at: number, last: StageMark): Record<RemoteStage, StageMark> =>
    Object.fromEntries(
      STAGES.map((s, i) => [s, i < at ? "done" : i === at ? last : "pending"]),
    ) as Record<RemoteStage, StageMark>;
  switch (status.state) {
    case "connecting":
      return marks(STAGES.indexOf(status.stage), "active");
    case "connected":
    case "resuming":
      return marks(STAGES.length, "done");
    case "reconnecting":
      return marks(STAGES.indexOf(status.error.stage), "failed");
    case "disconnected":
      return status.error
        ? marks(STAGES.indexOf(status.error.stage), "failed")
        : marks(-1, "pending");
    case "machineChanged":
      return marks(STAGES.indexOf("handshake"), "failed");
  }
}

/** 상태 한 줄(사람의 말). */
export function statusText(status: RemoteStatus, T: WelcomeStrings["remote"]): string {
  switch (status.state) {
    case "disconnected":
      if (status.error)
        return `${T.failed(T.stage[status.error.stage])} — ${T.failure[status.error.kind]}`;
      return status.byUser ? T.status.byUser : T.status.idle;
    case "connecting":
      return T.status.connecting(T.stage[status.stage]);
    case "connected":
      return T.status.connected;
    case "resuming":
      return T.status.resuming;
    case "reconnecting":
      return T.status.reconnecting(Math.ceil(status.retryInMs / 1000), status.attempt);
    case "machineChanged":
      return T.status.machineChanged(status.expected, status.found);
  }
}

/** 끊어도 Relay 가 그 머신의 실행 · 터미널을 쥐고 있는 상태인가(이어 붙기 시한 동안 — [끊기] 아래 안내, M7-09 리뷰 권고 7). */
export function holdsAfterDisconnect(status: RemoteStatus): boolean {
  return status.state === "reconnecting";
}

/** 그 링크에서 본 원격 Relay: 옛 판 데몬과 [옛 판 끝내기], 답하지 않는 옛 판, `pitwall` 명령 링크. */
function Relays({
  name,
  T,
  onError,
}: {
  name: string;
  T: WelcomeStrings["remote"];
  onError(message: string): void;
}) {
  const { client } = useHost();
  const relays = useRpcQuery("remote.relays", { name });
  const [said, setSaid] = useState<string>();
  const [busy, setBusy] = useState(false);
  const data = relays.data;
  if (!data || !client) return null;
  const failed = (method: string) => (e: unknown) => {
    onError(String(e instanceof Error ? e.message : e));
    console.warn(LOG, method, "failed", e);
  };
  const stop = (seen: RemoteOldRelay) => {
    setBusy(true);
    setSaid(undefined);
    // 보인 값 그대로 — Host 가 지금 원격과 견준다.
    client
      .call("remote.stopOld", { name, machine: data.machine, seen })
      .then((outcome) => setSaid(T.old.outcome[outcome.outcome]))
      .catch(failed("remote.stopOld"))
      .finally(() => {
        setBusy(false);
        void relays.refetch();
      });
  };
  const link = (on: boolean) => {
    setBusy(true);
    client
      .call("remote.linkCommand", { name, machine: data.machine, on })
      .catch(failed("remote.linkCommand"))
      .finally(() => {
        setBusy(false);
        void relays.refetch();
      });
  };
  return (
    <section
      aria-label={T.old.title}
      data-testid="remote-relays"
      className="flex flex-col gap-1 text-[12px]"
    >
      {data.old.map((old) => (
        <div key={old.file} data-old-relay={old.file} className="flex flex-wrap items-center gap-2">
          <span>{T.old.left(old.version ?? old.file, old.runs, old.terminals)}</span>
          {old.links > 0 && <span className="text-ui-accent">{T.old.links(old.links)}</span>}
          <button type="button" className={BUTTON} disabled={busy} onClick={() => stop(old)}>
            {old.links > 0 ? T.old.stopLinks(old.links) : T.old.stop}
          </button>
        </div>
      ))}
      {data.silent.map((silent) => (
        <div key={silent.file} data-silent-relay={silent.file}>
          {T.old.silent(silent.version ?? silent.file)}
        </div>
      ))}
      {said && <div role="status">{said}</div>}
      <label className="flex items-center gap-1">
        <input
          type="checkbox"
          checked={data.command === "linked"}
          disabled={busy || data.command === "other"}
          onChange={(e) => link(e.currentTarget.checked)}
        />
        {T.command}
      </label>
      {data.command === "other" && (
        <span className="text-ui-muted-foreground">{T.commandOther}</span>
      )}
    </section>
  );
}

/** 이 창이 그 호스트를 쓰는 동안 `remote.connect` 스트림을 열어 둔다. StrictMode 의 붙였다 떼기는 열기 · 닫기의 짝이다. */
function HostUse({ name, onStatus }: { name: string; onStatus(status: RemoteStatus): void }) {
  const { client } = useHost();
  useEffect(() => {
    if (!client) return;
    const abort = new AbortController();
    void (async () => {
      try {
        for await (const status of client.stream(
          "remote.connect",
          { name },
          { signal: abort.signal },
        )) {
          if (abort.signal.aborted) return;
          onStatus(status);
        }
      } catch (error) {
        if (!abort.signal.aborted) console.warn(LOG, "the connection stream ended", name, error);
      }
    })();
    return () => abort.abort();
  }, [client, name, onStatus]);
  return null;
}

function Stages({ status, T }: { status: RemoteStatus; T: WelcomeStrings["remote"] }) {
  const marks = stageMarks(status);
  return (
    <ol aria-label={T.stages} className="flex gap-2 text-[11px]">
      {STAGES.map((stage) => (
        <li
          key={stage}
          data-stage={stage}
          data-mark={marks[stage]}
          className={
            marks[stage] === "failed"
              ? "text-ui-accent"
              : marks[stage] === "pending"
                ? "text-ui-muted-foreground"
                : ""
          }
        >
          {{ done: "✓ ", active: "… ", failed: "✕ ", pending: "" }[marks[stage]]}
          {T.stage[stage]}
        </li>
      ))}
    </ol>
  );
}

function ConnectionLog({ name, T }: { name: string; T: WelcomeStrings["remote"] }) {
  const log = useRpcQuery("remote.log", { name });
  const { locales } = useI18n();
  const time = useMemo(
    () => new Intl.DateTimeFormat(locales[0], { timeStyle: "medium" }),
    [locales],
  );
  const lines = log.data ?? [];
  return (
    <section
      aria-label={T.logTitle(name)}
      data-testid="remote-log"
      className="max-h-[200px] overflow-y-auto border-t border-ui-border bg-ui-background px-2 py-1 font-mono text-[11px] whitespace-pre-wrap"
    >
      {lines.length === 0
        ? T.logEmpty
        : lines.map((line, i) => (
            <div key={i} className={line.level === "error" ? "text-ui-accent" : ""}>
              {`${time.format(line.at)} ${line.text}`}
            </div>
          ))}
    </section>
  );
}

function HostRow({
  host,
  status,
  using,
  logOpen,
  onConnect,
  onDisconnect,
  onRetry,
  onTrust,
  onToggleLog,
  onError,
}: {
  host: RemoteHostInfo;
  status: RemoteStatus;
  using: boolean;
  logOpen: boolean;
  onConnect(): void;
  onDisconnect(): void;
  onRetry(): void;
  onTrust(found: string): void;
  onToggleLog(): void;
  onError(message: string): void;
}) {
  const T = useWelcomeStrings().remote;
  const { locales } = useI18n();
  const when = (at: number) =>
    new Intl.DateTimeFormat(locales[0], { dateStyle: "short", timeStyle: "short" }).format(at);
  const target = `${host.user ? `${host.user}@` : ""}${host.host}${host.port ? `:${host.port}` : ""}`;
  const failed =
    (status.state === "disconnected" && status.error !== null) || status.state === "reconnecting";
  const live =
    status.state === "connected" ||
    status.state === "connecting" ||
    status.state === "reconnecting";
  return (
    <li
      data-remote-host={host.name}
      className="flex flex-col gap-1 border-b border-ui-border px-3 py-1.5"
    >
      <div className="flex items-center gap-2">
        <span className="w-[160px] flex-none truncate font-semibold">{host.name}</span>
        <span className="min-w-0 flex-1 truncate text-ui-muted-foreground" title={target}>
          {target}
        </span>
        <span className="flex-none rounded-sm border border-ui-border px-1 text-[11px]">
          {T.source[host.source]}
        </span>
      </div>
      <div className="flex items-center gap-3">
        <span role="status" data-testid="remote-status" className="min-w-0 flex-1">
          {statusText(status, T)}
        </span>
        {status.state === "machineChanged" ? (
          <button type="button" className={BUTTON} onClick={() => onTrust(status.found)}>
            {T.trust}
          </button>
        ) : using && live ? (
          <button type="button" className={BUTTON} onClick={onDisconnect}>
            {T.disconnect}
          </button>
        ) : using && (failed || (status.state === "disconnected" && status.byUser)) ? (
          <button type="button" className={BUTTON} onClick={onRetry}>
            {T.retry}
          </button>
        ) : (
          !using && (
            <button type="button" className={BUTTON} onClick={onConnect}>
              {T.connect}
            </button>
          )
        )}
        <button type="button" className={BUTTON} aria-expanded={logOpen} onClick={onToggleLog}>
          {logOpen ? T.hideLog : T.showLog}
        </button>
      </div>
      {using && live && holdsAfterDisconnect(status) && (
        <span className="text-[11px] text-ui-muted-foreground">{T.disconnectHolds}</span>
      )}
      <Stages status={status} T={T} />
      {using && status.state === "connected" && <Relays name={host.name} T={T} onError={onError} />}
      {host.lastLink && !using && (
        <span className="text-[11px] text-ui-muted-foreground">
          {T.last[host.lastLink.state](when(host.lastLink.at))}
        </span>
      )}
      {logOpen && <ConnectionLog name={host.name} T={T} />}
    </li>
  );
}

/** 「원격 호스트」 탭. */
export function RemoteHostsPanel() {
  const { client, key } = useHost();
  const queryClient = useQueryClient();
  const T = useWelcomeStrings().remote;
  const list = useRpcQuery("remote.hosts.list", null);
  const [using, setUsing] = useState<readonly string[]>([]);
  const [live, setLive] = useState<Readonly<Record<string, RemoteStatus>>>({});
  const [logOf, setLogOf] = useState<string>();
  const [error, setError] = useState<string>();

  // `remote.status` 는 owner 만의 토픽이라 Host 범위 묶음 구독(host.tsx)에 넣지 않고 따로 연다.
  useEffect(() => {
    if (!client) return;
    const invalidation = connectEventInvalidation(client, queryClient, null, [
      {
        topic: "remote.status",
        keys: [
          rpcQueryKey(key, "remote.hosts.list"),
          rpcQueryKey(key, "remote.log"),
          rpcQueryKey(key, "remote.relays"),
        ],
      },
      {
        topic: "remote.relays",
        keys: [rpcQueryKey(key, "remote.relays"), rpcQueryKey(key, "remote.log")],
      },
    ]);
    return () => invalidation.dispose();
  }, [client, key, queryClient]);

  const onStatus = useMemo(() => {
    const cache = new Map<string, (status: RemoteStatus) => void>();
    return (name: string) => {
      let f = cache.get(name);
      if (!f) {
        f = (status) => setLive((now) => ({ ...now, [name]: status }));
        cache.set(name, f);
      }
      return f;
    };
  }, []);

  const call = (method: "remote.disconnect" | "remote.retry", name: string) =>
    client?.call(method, { name }).catch((e: unknown) => {
      setError(String(e instanceof Error ? e.message : e));
      console.warn(LOG, method, "failed", e);
    });

  const hosts = list.data?.hosts ?? [];
  return (
    <section aria-label={T.label} className="flex min-h-0 flex-1 flex-col">
      <p className="flex-none border-b border-ui-border px-3 py-1 text-[12px] text-ui-muted-foreground">
        {T.addHint}
      </p>
      {using.map((name) => (
        <HostUse key={name} name={name} onStatus={onStatus(name)} />
      ))}
      <ul className="min-h-0 flex-1 overflow-y-auto">
        {hosts.length === 0 && list.isSuccess && (
          <li className="px-3 py-2 text-ui-muted-foreground">{T.empty}</li>
        )}
        {hosts.map((host) => {
          const isUsing = using.includes(host.name);
          const status = (isUsing && live[host.name]) || host.status;
          return (
            <HostRow
              key={host.name}
              host={host}
              status={status}
              using={isUsing}
              logOpen={logOf === host.name}
              onConnect={() => {
                setError(undefined);
                setUsing((now) => (now.includes(host.name) ? now : [...now, host.name]));
              }}
              onDisconnect={() => void call("remote.disconnect", host.name)}
              onRetry={() => void call("remote.retry", host.name)}
              onTrust={(machine) =>
                void client
                  ?.call("remote.trustMachine", { name: host.name, machine })
                  .catch((e: unknown) => setError(String(e instanceof Error ? e.message : e)))
              }
              onToggleLog={() => setLogOf((now) => (now === host.name ? undefined : host.name))}
              onError={setError}
            />
          );
        })}
      </ul>
      {error && (
        <div
          role="alert"
          className="flex-none truncate border-t border-ui-accent px-3 py-0.5 text-[12px]"
        >
          {error}
        </div>
      )}
    </section>
  );
}

/** 원격 호스트를 붙이는 ssh 의 질문(호스트 키 확인 · 암호). 웰컴 창이 떠 있는 동안 듣는다. 답의 값은 남기지 않는다. */
export function RemotePrompts() {
  const { client } = useHost();
  const [queue, setQueue] = useState<readonly AskpassPrompt[]>([]);
  useEffect(() => {
    if (!client) return;
    setQueue([]);
    const abort = new AbortController();
    void (async () => {
      try {
        for await (const event of client.stream("remote.prompts", null, { signal: abort.signal })) {
          if (abort.signal.aborted) return;
          if (event.kind === "closed") {
            setQueue((q) => q.filter((p) => p.promptId !== event.promptId));
          } else if (event.prompt.request.kind === "askpass") {
            const { promptId, request } = event.prompt;
            setQueue((q) => [...q, { promptId, text: request.text, hint: request.hint }]);
          }
        }
      } catch (error) {
        if (!abort.signal.aborted) console.warn(LOG, "stopped listening for remote prompts", error);
      }
    })();
    return () => abort.abort();
  }, [client]);
  const current = queue[0];
  if (!current || !client) return null;
  const answer = (reply: GitPromptAnswer) => {
    setQueue((q) => q.filter((p) => p.promptId !== current.promptId));
    client
      .call("remote.prompt.answer", { promptId: current.promptId, answer: reply })
      .catch((error: unknown) =>
        console.warn(LOG, "could not answer the remote prompt", current.promptId, error),
      );
  };
  return <AskpassDialog key={current.promptId} prompt={current} onAnswer={answer} />;
}
