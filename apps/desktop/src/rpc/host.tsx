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
 * 다시 읽히거나 읽기가 실패하면 `config.get` 을 다시 부른다(crates/pw-core/src/config/store.rs 의 토픽).
 */
export function hostInvalidations(host: string): readonly InvalidationRule[] {
  const keys = [rpcQueryKey(host, "config.get")];
  return [
    { topic: "config.changed", keys },
    { topic: "config.error", keys },
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

/** 이 창의 Host 에 unary 메서드를 쿼리로 부른다. 연결되기 전에는 부르지 않고 기다린다. */
export function useRpcQuery<M extends UnaryMethod>(method: M, params: UnaryMethods[M]["params"]) {
  const { client, key } = useHost();
  return useQuery(
    client
      ? rpcQueryOptions(client, key, method, params)
      : { queryKey: rpcQueryKey(key, method, params ?? null), queryFn: skipToken },
  );
}
