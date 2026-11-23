// 다른 머신 문서의 언어 서버 길(18-m7-plan M7-21 ⑥ — M7-13 이 넘김, 메인 판정). 원격 탭의 문서는 그 머신의 언어 서버로 연다: 문서
// RPC(`lsp.document.*`) · 기능 행의 RPC · 프로젝트 이벤트(`lsp.diagnostics` · `lsp.status`)가 모두 그 머신 연결로, 그 머신의 프로젝트
// id 로 간다. 창의 이 머신 연결로 다른 머신의 경로를 보내지 않는다(이 머신 Host 는 그 경로를 모른다 — 같은 경로의 이 머신 파일로
// 새면 엉뚱한 서버가 답한다).
//
// - 머신의 프로젝트 id 는 창의 묶음에서 곧바로 안다(`CheckoutHosts.connection(machine).projectId`). 묶음 멤버가 아닌 머신은 길이 없다.
// - 부를 때 그 머신이 붙을 때까지 기다린다(`CheckoutHosts.client` — 그 머신의 탭이 머신을 쥐고 있다, project-window.tsx
//   `useTabMachines`). 붙지 못하면 그 요청이 실패한다(기능 행이 조용히 비운다 — 결정 M4-D8).
// - 이벤트 구독은 그 머신의 연결이 생기거나 바뀔 때 붙인다(project-subscriptions.ts — 생기기 전에 건 구독은 기다린다).
// - 창 전역 표(편집 · 상태 — edit-window.ts · status-window.ts)는 창의 이 머신 `services` 로 찾는다 — 다른 머신 문서의 `services` 는
//   그 표에 없으므로 여러 파일 편집(rename · 코드 액션의 workspace edit) · 상태바 · 설치 배너는 그 문서에서 붙지 않는다(이 행에서
//   하지 않음 — 보고서 「5」).

import type { RpcClient } from "@pitwall/rpc-client";
import type { StoreApi } from "zustand/vanilla";

import type { LspRoute, LspServices, LspSource } from "./lsp-sessions";
import { projectSubscriptions } from "./project-subscriptions";

/** 창의 체크아웃별 연결에서 이 길이 쓰는 것(checkout-hosts.tsx `CheckoutHosts`). */
export interface MachineHosts {
  readonly store: Pick<StoreApi<unknown>, "subscribe">;
  connection(machine: string): { client: RpcClient | undefined; projectId: string | undefined };
  client(machine: string): Promise<{ client: RpcClient; projectId: string }>;
}

/** 머신마다 길 하나(같은 머신의 문서는 같은 구독 · 같은 연결). */
export function machineLspRoutes(
  hosts: () => MachineHosts | undefined,
  /** 머신과 상관없는 것(링크 열기 · 문장 · 조각 강조 — 강조는 이 머신 Host 가 한다, 08 「로컬 / 원격 역할 분담」). */
  shared: Omit<LspServices, "call" | "subscribe">,
): (machine: string) => LspRoute | undefined {
  const routes = new Map<string, LspRoute>();
  return (machine) => {
    const h = hosts();
    const projectId = h?.connection(machine).projectId;
    if (!h || projectId === undefined) return undefined;
    const key = `${machine}\u0000${projectId}`;
    const known = routes.get(key);
    if (known) return known;
    const call = async <T>(run: (client: RpcClient) => Promise<T>): Promise<T> =>
      run((await h.client(machine)).client);
    const subscriptions = projectSubscriptions(projectId);
    let connected: RpcClient | undefined;
    const follow = () => {
      const now = h.connection(machine).client;
      if (now === connected) return;
      connected = now;
      subscriptions.connect(now);
    };
    h.store.subscribe(follow);
    follow();
    const source: LspSource = {
      open: (params) => call((c) => c.call("lsp.document.open", params)),
      change: (params) => call((c) => c.call("lsp.document.change", params)),
      save: (params) => call((c) => c.call("lsp.document.save", params)),
      close: (params) => call((c) => c.call("lsp.document.close", params)),
      capabilities: (params) => call((c) => c.call("lsp.document.capabilities", params)),
    };
    const services: LspServices = {
      ...shared,
      call: (method, params, options) => call((c) => c.call(method, params, options)),
      subscribe: (topics, listener) => subscriptions.subscribe(topics, listener),
    };
    const route = { projectId, source, services };
    routes.set(key, route);
    return route;
  };
}
