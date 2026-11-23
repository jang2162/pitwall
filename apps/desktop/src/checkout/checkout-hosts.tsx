// 창의 체크아웃별 연결(18-m7-plan M7-11 ① — 「공유 부품의 주인」 표의 `useCheckoutHost`). 묶음 창은 머신마다 연결을 하나씩 둔다:
//
// - **이 머신**은 창의 연결 그대로다(`HostProvider` 가 연 프로젝트 연결 — 프로젝트 상태 · 이 머신의 기능).
// - **다른 머신**은 그 머신의 묶음 멤버로 연 연결이다(`{ member: { host: 머신 id, project: 그 머신의 프로젝트 id } }` — Rust 가 그 창의
//   묶음 멤버인지 보고 그 머신 프로젝트의 `Project` 범위로 연다, src-tauri/src/ipc.rs `window_admits`). git 쓰기 · 파일 쓰기가 그
//   머신의 `scope_may_write` 를 지난다.
// - **연결은 쓰는 머신만 연다**(08 「창과 활성 체크아웃」, D56): 활성 체크아웃의 머신과 탭이 열린 머신. 쓰는 동안(`retain`) 공급자가
//   **따로 연 로컬 Host 범위 연결**(`{ hostId: "local" }` — 창의 연결은 프로젝트 범위라 `remote.*` 를 받지 못한다)로 `remote.connect`
//   스트림을 열어 링크를 쥔다 — `remote.*` 는 Host 범위 연결만 받으므로(M7-08) 프로젝트
//   연결로 넓히지 않고 창이 Host 연결을 따로 연다(M7-08 리뷰 R6, 메인 판정). 스트림이 「붙음」 을 알리면 멤버 연결을 열고, 끊기면
//   닫는다. 아무도 쓰지 않으면 스트림을 닫는다 — 그 머신을 쓰는 것이 모두 닫히면 Host 가 링크를 닫는다.
// - 머신의 호스트 이름(`remote.connect` 의 `name`)과 별명은 `remote.hosts.list` 의 기억한 머신 id 로 찾는다. 모르면 스트림 없이
//   멤버 연결만 연다(이미 붙은 링크).

import type { ProjectBundle, RemoteHostList, RemoteStatus } from "@pitwall/rpc-client";
import { RpcClient } from "@pitwall/rpc-client";
import { createContext, type ReactNode, useContext, useEffect, useMemo, useState } from "react";
import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import { useActiveMachine } from "../open-with/active-worktree";
import {
  type HostConnection,
  HostConnectionProvider,
  useHost,
  useOptionalHost,
  useRpcQueryOn,
} from "../rpc/host";
import { hostKey } from "../rpc/queries";
import { connectHost, type HostTarget, type TauriConnection } from "../tauri-transport";
import { LOCAL_MACHINE } from "./checkout";

const LOG = "[checkout]";

/** 머신 하나의 연결. 다른 머신이 아직 붙지 않았으면 `client` 가 없다. */
export interface CheckoutConnection extends HostConnection {
  machine: string;
  /** 그 머신 레지스트리의 프로젝트 id. 묶음을 아직 읽지 못했으면 없다. */
  projectId: string | undefined;
  /** 다른 머신의 링크 상태(`remote.connect`). 이 머신 · 모르면 없다. */
  status?: RemoteStatus;
}

/** 묶음 창이 아는 머신들 — 묶음의 멤버(머신 → 프로젝트 id)와 호스트 이름(머신 → `remote.*` 의 이름). */
export interface CheckoutDirectory {
  members: ReadonlyMap<string, string>;
  hostNames: ReadonlyMap<string, string>;
}

/** 창 id(`?project=` — 로컬 프로젝트 id 또는 묶음 id)의 묶음. */
export function windowBundle(
  bundles: readonly ProjectBundle[] | undefined,
  windowId: string | undefined,
): ProjectBundle | undefined {
  if (windowId === undefined) return undefined;
  return bundles?.find(
    (b) =>
      b.id === windowId ||
      b.members.some((m) => m.machine === LOCAL_MACHINE && m.projectId === windowId),
  );
}

/** 머신 id → 호스트 이름(별명). 한 머신을 두 이름이 기억하면 앞의 것(M7-08 이 둘째 이름의 연결을 거절한다). */
export function hostNamesOf(list: RemoteHostList | undefined): Map<string, string> {
  const out = new Map<string, string>();
  for (const h of list?.hosts ?? [])
    if (h.machine && !out.has(h.machine)) out.set(h.machine, h.name);
  return out;
}

interface Remote {
  users: number;
  /** 지금 도는 붙기(스트림 · 멤버 연결). 없으면 멈춤. */
  running?: { abort: AbortController; member?: TauriConnection };
  client?: RpcClient;
  status?: RemoteStatus;
}

interface State {
  local: HostConnection | undefined;
  /** 공급자가 따로 연 로컬 Host 범위 연결 — `remote.connect` · `remote.hosts.list` 를 부른다. */
  link: RpcClient | undefined;
  directory: CheckoutDirectory;
  remote: Readonly<Record<string, Readonly<Omit<Remote, "running">>>>;
}

export interface CheckoutHosts {
  readonly store: StoreApi<State>;
  setLocal(local: HostConnection): void;
  setLink(link: RpcClient | undefined): void;
  setDirectory(directory: CheckoutDirectory): void;
  /** 그 머신을 쓰기 시작한다(연결을 연다). 돌려준 함수로 놓는다 — 마지막으로 놓으면 닫는다. */
  retain(machine: string): () => void;
  /** 그 머신의 지금 연결. */
  connection(machine: string): CheckoutConnection;
  /**
   * 그 머신의 클라이언트와 프로젝트 id — 붙을 때까지 `waitMs` 기다린다(문서 읽기 · 쓰기). 넘으면 거절한다. 부르는 쪽이 그 머신을
   * `retain` 하고 있어야 붙는다.
   */
  client(machine: string, waitMs?: number): Promise<{ client: RpcClient; projectId: string }>;
  /** 창이 사는 동안의 짝(StrictMode): 멈추면 모든 붙기를 닫고, 다시 시작하면 쓰던 머신을 다시 붙인다. */
  start(): void;
  stop(): void;
}

export const CLIENT_WAIT_MS = 30_000;

export function createCheckoutHosts(
  connect: (target: HostTarget) => Promise<TauriConnection> = connectHost,
): CheckoutHosts {
  const remotes = new Map<string, Remote>();
  let started = false;
  const store = createStore<State>()(() => ({
    local: undefined,
    link: undefined,
    directory: { members: new Map(), hostNames: new Map() },
    remote: {},
  }));

  function publish() {
    const remote: Record<string, Omit<Remote, "running">> = {};
    for (const [machine, r] of remotes)
      remote[machine] = { users: r.users, client: r.client, status: r.status };
    store.setState({ remote });
  }

  function memberTarget(machine: string): HostTarget | undefined {
    const project = store.getState().directory.members.get(machine);
    return project === undefined ? undefined : { member: { host: machine, project } };
  }

  function closeMember(r: Remote) {
    const member = r.running?.member;
    if (r.running) r.running.member = undefined;
    r.client = undefined;
    if (member) void member.close();
  }

  async function openMember(machine: string, r: Remote, run: NonNullable<Remote["running"]>) {
    const target = memberTarget(machine);
    if (!target || run.member || run.abort.signal.aborted) return;
    try {
      const conn = await connect(target);
      if (run.abort.signal.aborted || r.running !== run) {
        void conn.close();
        return;
      }
      run.member = conn;
      r.client = new RpcClient(conn.transport);
      conn.transport.onClose?.(() => {
        if (run.member !== conn) return;
        run.member = undefined;
        r.client = undefined;
        publish();
      });
      publish();
    } catch (e) {
      console.warn(LOG, "could not open the checkout connection", machine, e);
    }
  }

  /** 쓰는 머신이고 아직 멈춰 있으면 붙기를 시작한다(로컬 연결 · 묶음을 읽은 뒤에). */
  function run(machine: string) {
    const r = remotes.get(machine);
    const local = store.getState().link;
    if (!r || r.users === 0 || r.running || !started || !local || !memberTarget(machine)) return;
    const running = { abort: new AbortController() } as NonNullable<Remote["running"]>;
    r.running = running;
    const name = store.getState().directory.hostNames.get(machine);
    if (name === undefined) {
      void openMember(machine, r, running);
      return;
    }
    void (async () => {
      try {
        for await (const status of local.stream(
          "remote.connect",
          { name },
          { signal: running.abort.signal },
        )) {
          if (running.abort.signal.aborted) return;
          r.status = status;
          if (status.state === "connected") void openMember(machine, r, running);
          // 링크가 이어 붙는 동안(`resuming` — M7-09 ①)은 멤버 연결을 그대로 둔다: 채널 · 스트림은 링크가 이어 붙이면 그대로 산다.
          // 그 밖(`reconnecting` 은 링크가 이미 닫혀 새로 붙기를 기다리는 백오프다 · 끊김 · 다른 머신)은 닫는다 — 다시 붙으면 새
          // 연결을 열고 실행 · 터미널은 `since` 로 이어 받는다(18-m7-plan M7-14 ⑤, 메인 판정 2026-11-23).
          else if (status.state !== "resuming") closeMember(r);
          publish();
        }
      } catch (e) {
        if (!running.abort.signal.aborted) console.warn(LOG, "the link stream ended", machine, e);
      }
    })();
  }

  function halt(r: Remote) {
    r.running?.abort.abort();
    closeMember(r);
    r.running = undefined;
    r.status = undefined;
  }

  function refresh() {
    for (const machine of remotes.keys()) run(machine);
    publish();
  }

  const hosts: CheckoutHosts = {
    store,
    setLocal(local) {
      store.setState({ local });
      refresh();
    },
    setLink(link) {
      // 연결이 바뀌면(닫힘 · 다시 열림) 그 연결로 연 스트림은 끝났다 — 쓰던 머신을 새 연결로 다시 붙인다.
      for (const r of remotes.values()) halt(r);
      store.setState({ link });
      refresh();
    },
    setDirectory(directory) {
      store.setState({ directory });
      refresh();
    },
    retain(machine) {
      if (machine === LOCAL_MACHINE) return () => {};
      const r = remotes.get(machine) ?? { users: 0 };
      remotes.set(machine, r);
      r.users++;
      run(machine);
      publish();
      let released = false;
      return () => {
        if (released) return;
        released = true;
        r.users--;
        if (r.users === 0) {
          halt(r);
          remotes.delete(machine);
        }
        publish();
      };
    },
    connection(machine) {
      const { local, directory, remote } = store.getState();
      if (machine === LOCAL_MACHINE) {
        const own = local ?? { target: { hostId: LOCAL_MACHINE }, key: "", client: undefined };
        const projectId =
          directory.members.get(LOCAL_MACHINE) ??
          ("projectId" in own.target ? own.target.projectId : undefined);
        return { ...own, machine, projectId };
      }
      const project = directory.members.get(machine);
      const target: HostTarget =
        project === undefined ? { hostId: machine } : { member: { host: machine, project } };
      const r = remote[machine];
      return {
        target,
        key: hostKey(target),
        client: r?.client,
        machine,
        projectId: project,
        status: r?.status,
      };
    },
    client(machine, waitMs = CLIENT_WAIT_MS) {
      const ready = () => {
        const c = hosts.connection(machine);
        return c.client && c.projectId !== undefined
          ? { client: c.client, projectId: c.projectId }
          : undefined;
      };
      const now = ready();
      if (now) return Promise.resolve(now);
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          off();
          reject(new Error(`the machine ${machine} is not connected`));
        }, waitMs);
        const off = store.subscribe(() => {
          const got = ready();
          if (!got) return;
          clearTimeout(timer);
          off();
          resolve(got);
        });
      });
    },
    start() {
      started = true;
      refresh();
    },
    stop() {
      started = false;
      for (const r of remotes.values()) halt(r);
      publish();
    },
  };
  return hosts;
}

const CheckoutHostsContext = createContext<CheckoutHosts | undefined>(undefined);
const LinkHostContext = createContext<HostConnection | undefined>(undefined);

const HOST_SCOPE: HostTarget = { hostId: LOCAL_MACHINE };

/**
 * 이 머신 Host 의 **Host 범위** 연결을 창이 사는 동안 연다(M7-11 리뷰 B1 — `remote.*` 는 Host 범위만 받는다, M7-08). 닫히면 없앤다.
 * StrictMode 의 붙였다 떼기는 열기 · 닫기의 짝이다. **닫힌 뒤 다시 열지 않는다** — 이 머신 Host 와의 IPC 연결이 닫히는 것은 창 ·
 * 페이지가 사라질 때뿐이라(src-tauri/src/ipc.rs) 다시 열 자리가 없다. 다른 길로 닫히면 다른 머신은 창을 다시 열 때까지 붙지 않는다.
 */
function useHostScopeConnection(
  connect: (target: HostTarget) => Promise<TauriConnection>,
): HostConnection {
  const [client, setClient] = useState<RpcClient>();
  useEffect(() => {
    let disposed = false;
    let close: (() => void) | undefined;
    connect(HOST_SCOPE)
      .then((conn) => {
        const opened = new RpcClient(conn.transport);
        close = () => {
          opened.close();
          void conn.close();
        };
        if (disposed) return close();
        conn.transport.onClose?.(() => !disposed && setClient(undefined));
        setClient(opened);
      })
      .catch((e: unknown) => console.warn(LOG, "could not open the host connection", e));
    return () => {
      disposed = true;
      close?.();
      setClient(undefined);
    };
  }, [connect]);
  return useMemo(
    () => ({ target: HOST_SCOPE, key: `link:${hostKey(HOST_SCOPE)}`, client }),
    [client],
  );
}

/** 창의 로컬 **Host 범위** 연결(`remote.*` 를 부른다). 체크아웃 공급자 밖이면 `useLocalHost`(웰컴 · 설정 창은 Host 범위다). */
export function useLinkHost(): HostConnection {
  const own = useContext(LinkHostContext);
  const local = useLocalHost();
  return own ?? local;
}
const LocalHostContext = createContext<HostConnection | undefined>(undefined);

/**
 * 창의 체크아웃별 연결을 둔다. `windowId` 는 창 쿼리의 id(로컬 프로젝트 id 또는 묶음 id). 창의 연결(`HostProvider`) 안에 둔다 — 그
 * 연결이 이 머신의 연결이고 `remote.connect` 를 부르는 Host 범위 연결이다. `connect` 는 시험이 가짜를 넘긴다.
 */
export function CheckoutHostsProvider({
  windowId,
  connect,
  children,
}: {
  windowId: string | undefined;
  connect?: (target: HostTarget) => Promise<TauriConnection>;
  children: ReactNode;
}) {
  const local = useHost();
  const [hosts] = useState(() => createCheckoutHosts(connect));
  const link = useHostScopeConnection(connect ?? connectHost);
  const bundles = useRpcQueryOn(local, "bundle.list", null, { retry: false });
  const names = useRpcQueryOn(link, "remote.hosts.list", null, { retry: false });
  const bundle = windowBundle(bundles.data, windowId);
  useEffect(() => {
    hosts.start();
    return () => hosts.stop();
  }, [hosts]);
  useEffect(() => hosts.setLocal(local), [hosts, local]);
  useEffect(() => hosts.setLink(link.client), [hosts, link.client]);
  useEffect(() => {
    hosts.setDirectory({
      members: new Map(bundle?.members.map((m) => [m.machine, m.projectId])),
      hostNames: hostNamesOf(names.data),
    });
  }, [hosts, bundle, names.data]);
  return (
    <LocalHostContext value={local}>
      <LinkHostContext value={link}>
        <CheckoutHostsContext value={hosts}>{children}</CheckoutHostsContext>
      </LinkHostContext>
    </LocalHostContext>
  );
}

/** 창의 체크아웃별 연결 모델. 공급자 밖이면 `undefined`(웰컴 · 설정 창). */
export function useCheckoutHosts(): CheckoutHosts | undefined {
  return useContext(CheckoutHostsContext);
}

const NO_HOST: HostConnection = { target: { hostId: LOCAL_MACHINE }, key: "", client: undefined };

/**
 * 창의 이 머신 연결. 체크아웃 공급자 밖이면 문맥의 연결(웰컴 · 설정 창은 그것이 로컬이다). 연결이 아예 없으면(부품 시험) 클라이언트
 * 없는 연결 — 그 위의 쿼리는 부르지 않는다.
 */
export function useLocalHost(): HostConnection {
  const own = useContext(LocalHostContext);
  const context = useOptionalHost();
  return own ?? context ?? NO_HOST;
}

/**
 * 그 머신 체크아웃의 연결(「공유 부품의 주인」 표의 `useCheckoutHost(checkout)`). 그리는 동안 그 머신을 쓴다 — 다른 머신이면 링크를
 * 쥐고 멤버 연결을 연다. 체크아웃 공급자 밖이면 문맥의 연결이다.
 */
export function useCheckoutHost(checkout: { machine: string } | string): CheckoutConnection {
  const machine = typeof checkout === "string" ? checkout : checkout.machine;
  const hosts = useCheckoutHosts();
  const context = useHost();
  useEffect(() => hosts?.retain(machine), [hosts, machine]);
  const remote = useStore(hosts?.store ?? FALLBACK, (s) => s.remote[machine]);
  const local = useStore(hosts?.store ?? FALLBACK, (s) => s.local);
  const directory = useStore(hosts?.store ?? FALLBACK, (s) => s.directory);
  return useMemo(() => {
    if (!hosts) {
      const projectId = "projectId" in context.target ? context.target.projectId : undefined;
      return { ...context, machine, projectId };
    }
    return hosts.connection(machine);
    // 셋은 `connection` 이 읽는 store 칸이다 — 바뀌면 다시 만든다.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hosts, machine, context, remote, local, directory]);
}

const FALLBACK = createStore<State>()(() => ({
  local: undefined,
  link: undefined,
  directory: { members: new Map(), hostNames: new Map() },
  remote: {},
}));

/** 활성 체크아웃의 연결을 그 아래 기능의 문맥(`useHost`)으로 둔다. */
export function ActiveCheckoutHost({
  children,
}: {
  children: (active: CheckoutConnection) => ReactNode;
}) {
  const active = useCheckoutHost(useActiveMachine());
  return <HostConnectionProvider connection={active}>{children(active)}</HostConnectionProvider>;
}
