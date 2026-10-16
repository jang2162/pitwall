// `@pitwall/rpc-client` 의 unary 호출을 TanStack Query 의 쿼리로 감싼다.
//
// 쿼리 키는 `["rpc", host, method, params]` 다. 접두사로 무효화하므로 `["rpc", host, method]` 는 그 메서드의 모든 params 를,
// `["rpc", host]` 는 그 Host 의 모든 응답을 가리킨다. host 를 키에 넣는 것은 한 창이 여러 Host(원격 포함)를 볼 때
// 같은 메서드의 응답이 섞이지 않게 하려는 것이다.

import type { RpcClient, UnaryMethods } from "@pitwall/rpc-client";
import type { QueryKey } from "@tanstack/react-query";

import type { HostTarget } from "../tauri-transport";

export type UnaryMethod = keyof UnaryMethods & string;
type Params<M extends UnaryMethod> = UnaryMethods[M]["params"];
type Result<M extends UnaryMethod> = UnaryMethods[M]["result"];

/** 쿼리 키 안의 Host 자리. */
export function hostKey(target: HostTarget): string {
  return "hostId" in target ? `host:${target.hostId}` : `project:${target.projectId}`;
}

export function rpcQueryKey(host: string, method?: UnaryMethod, params?: unknown): QueryKey {
  if (method === undefined) return ["rpc", host];
  if (params === undefined) return ["rpc", host, method];
  return ["rpc", host, method, params];
}

/**
 * `useQuery` · `queryClient.fetchQuery` 에 그대로 넘기는 옵션. 쿼리가 버려지면(언마운트 · 취소) TanStack 이 signal 을 끊고,
 * 클라이언트가 `request.cancel` 을 보낸다.
 */
export function rpcQueryOptions<M extends UnaryMethod>(
  client: RpcClient,
  host: string,
  method: M,
  params: Params<M>,
) {
  return {
    queryKey: rpcQueryKey(host, method, params ?? null),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<Result<M>> =>
      // Args 는 params 가 null 일 수 있는 메서드에서만 생략을 허락한다. 여기서는 늘 넘기므로 단언으로 맞춘다.
      (client.call as (m: M, p: Params<M>, o: { signal: AbortSignal }) => Promise<Result<M>>)(
        method,
        params,
        { signal },
      ),
  };
}
