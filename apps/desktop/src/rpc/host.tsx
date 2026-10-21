// 창 하나가 Host 하나에 여는 연결과, 그 연결로 부르는 쿼리.

import { RpcClient, type UnaryMethods } from "@pitwall/rpc-client";
import { skipToken, useQuery, useQueryClient } from "@tanstack/react-query";
import { createContext, type ReactNode, useContext, useEffect, useMemo, useState } from "react";

import { connectHost, type HostTarget } from "../tauri-transport";
import { connectEventInvalidation, type InvalidationRule } from "./invalidation";
import { hostKey, rpcQueryKey, rpcQueryOptions, type UnaryMethod } from "./queries";

const LOG = "[host]";

/**
 * 프로젝트 범위 이벤트 → 쿼리 무효화 규칙. 이벤트를 내는 기능이 그 쿼리와 함께 여기에 더한다. 프로젝트 창이 프로젝트 id 로
 * 연결하게 되면(M0-12) 그 창에서 구독이 열린다.
 */
export const RPC_INVALIDATIONS: readonly InvalidationRule[] = [];

/**
 * Host 범위 이벤트 → 쿼리 무효화 규칙. 모든 창이 프로젝트와 무관하게 구독한다. 전역 설정(`$CONFIG`, keymap.toml 포함)이
 * 다시 읽히거나 읽기가 실패하면 `config.get` 과 UI 언어(`i18n.get`, settings.toml 의 `ui.language`)를 다시 부른다(crates/pw-core/src/config/store.rs 의 토픽). 프로젝트
 * 레지스트리가 바뀌면(만들기 · 고치기 · 지우기 · 열기 · 신뢰, crates/pw-core/src/registry/mod.rs 의 `TOPIC_CHANGED`) 프로젝트
 * 목록을 다시 부른다 — 다른 창 · CLI 가 바꾼 것도 웰컴 창에 보인다.
 */
export function hostInvalidations(host: string): readonly InvalidationRule[] {
  const config = [rpcQueryKey(host, "config.get"), rpcQueryKey(host, "i18n.get")];
  const projects = [
    rpcQueryKey(host, "project.list"),
    rpcQueryKey(host, "project.recent"),
    rpcQueryKey(host, "project.get"),
    rpcQueryKey(host, "project.resolve"),
    rpcQueryKey(host, "project.trust"),
  ];
  // 시간을 넘겼던 로그인 셸이 백그라운드 재시도로 살아나면 Host 범위에도 `runtime.changed`(loginShell)가 온다 — 설정 창의
  // 「Orca 에서 Pitwall 열기」 검사(`cli.status`)가 로그인 셸 PATH 를 다시 본다(crates/pw-runtime/src/runtime.rs).
  const runtime = [rpcQueryKey(host, "cli.status"), rpcQueryKey(host, "runtime.get")];
  return [
    { topic: "config.changed", keys: config },
    { topic: "config.error", keys: config },
    { topic: "project.changed", keys: projects },
    { topic: "runtime.changed", keys: runtime },
  ];
}

export interface HostConnection {
  target: HostTarget;
  /** 쿼리 키의 Host 자리(queries.ts). */
  key: string;
  /** 연결되기 전 · 실패한 뒤에는 없다. */
  client: RpcClient | undefined;
}

const HostContext = createContext<HostConnection | undefined>(undefined);

/** `target` 은 모듈 상수로 넘긴다. 렌더마다 새 객체를 주면 렌더마다 연결을 다시 연다. */
export function HostProvider({
  target,
  invalidations = RPC_INVALIDATIONS,
  hostScopeInvalidations = hostInvalidations,
  children,
}: {
  target: HostTarget;
  invalidations?: readonly InvalidationRule[];
  /** 모듈 상수(함수)로 넘긴다 — 렌더마다 새 함수를 주면 렌더마다 구독을 다시 연다. */
  hostScopeInvalidations?: (host: string) => readonly InvalidationRule[];
  children: ReactNode;
}) {
  const [client, setClient] = useState<RpcClient>();
  const queryClient = useQueryClient();
  const key = hostKey(target);

  useEffect(() => {
    let disposed = false;
    let close: (() => void) | undefined;
    connectHost(target)
      .then((conn) => {
        const opened = new RpcClient(conn.transport);
        close = () => {
          opened.close();
          void conn.close();
        };
        if (disposed) {
          close();
          return;
        }
        setClient(opened);
      })
      .catch((e: unknown) => console.error(LOG, "connect failed", e));
    return () => {
      disposed = true;
      close?.();
      setClient(undefined);
    };
  }, [target]);

  useEffect(() => {
    if (!client || !("projectId" in target)) return;
    const invalidation = connectEventInvalidation(
      client,
      queryClient,
      target.projectId,
      invalidations,
    );
    return () => invalidation.dispose();
  }, [client, target, queryClient, invalidations]);

  useEffect(() => {
    if (!client) return;
    const invalidation = connectEventInvalidation(
      client,
      queryClient,
      null,
      hostScopeInvalidations(key),
    );
    return () => invalidation.dispose();
  }, [client, key, queryClient, hostScopeInvalidations]);

  const value = useMemo(() => ({ target, key, client }), [target, key, client]);
  return <HostContext value={value}>{children}</HostContext>;
}

export function useHost(): HostConnection {
  const host = useContext(HostContext);
  if (!host) throw new Error("HostProvider 밖에서 useHost 를 불렀다");
  return host;
}

/** `useRpcQuery` 가 TanStack Query 에 그대로 넘기는 선택 옵션. */
export interface RpcQueryOptions {
  staleTime?: number;
  retry?: boolean | number;
}

/** 이 창의 Host 에 unary 메서드를 쿼리로 부른다. 연결되기 전에는 부르지 않고 기다린다. */
export function useRpcQuery<M extends UnaryMethod>(
  method: M,
  params: UnaryMethods[M]["params"],
  options: RpcQueryOptions = {},
) {
  const { client, key } = useHost();
  return useQuery(
    client
      ? { ...rpcQueryOptions(client, key, method, params), ...options }
      : { queryKey: rpcQueryKey(key, method, params ?? null), queryFn: skipToken },
  );
}
