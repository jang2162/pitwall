// 다른 머신 문서의 끊김 · 다시 붙기 판정(machine-documents.ts 머리 주석): 멤버 연결이 살아 있어도 링크가 `connected` 가 아니면
// 끊김이다(M7-09 — 다시 붙는 중 · 이어 붙는 중에는 채널이 그대로라 연결이 남는다, M7-14). 처음 붙는 것에는 맞추지 않는다.

import type { RemoteStatus, RpcClient } from "@pitwall/rpc-client";
import { describe, expect, it, vi } from "vitest";
import { createStore } from "zustand/vanilla";

import type { CheckoutConnection, CheckoutHosts } from "../checkout/checkout-hosts";
import type { DocumentStore } from "./documents";
import { startMachineDocumentSync } from "./machine-documents";

const MINI = "SHA256:mini";

function fakeHosts() {
  const client = {
    subscribe: vi.fn(async () => ({ unsubscribe: async () => {} })),
  } as unknown as RpcClient;
  const store = createStore<{
    remote: Record<string, { client?: RpcClient; status?: RemoteStatus }>;
  }>()(() => ({ remote: {} }));
  const hosts = {
    store,
    connection: (machine: string): CheckoutConnection => {
      const r = store.getState().remote[machine];
      return {
        target: { member: { host: machine, project: "r1" } },
        key: "",
        client: r?.client,
        machine,
        projectId: "r1",
        status: r?.status,
      };
    },
  } as unknown as CheckoutHosts;
  return { hosts, client, store };
}

function fakeDocuments() {
  return {
    store: createStore(() => ({ docs: {} })),
    opened: () => [
      { path: `pitwall-remote://${encodeURIComponent(MINI)}/work/a.ts`, worktree: undefined },
    ],
    disconnect: vi.fn(),
    reconnect: vi.fn(async () => {}),
    changed: vi.fn(async () => {}),
  } as unknown as DocumentStore & {
    disconnect: ReturnType<typeof vi.fn>;
    reconnect: ReturnType<typeof vi.fn>;
  };
}

const CONNECTED: RemoteStatus = { state: "connected", machine: MINI };
const RECONNECTING: RemoteStatus = {
  state: "reconnecting",
  attempt: 1,
  retryInMs: 200,
  error: { kind: "io", message: "lost" } as never,
};

describe("startMachineDocumentSync", () => {
  it("counts a machine whose connection lives on while its link reconnects as dropped", () => {
    const { hosts, client, store } = fakeHosts();
    const documents = fakeDocuments();
    const watch = vi.fn(() => () => {});
    const stop = startMachineDocumentSync(hosts, documents, watch);
    store.setState({ remote: { [MINI]: { client, status: CONNECTED } } });
    expect(documents.disconnect).not.toHaveBeenCalled();
    // 처음 본 때 이미 붙어 있으면 맞추지 않는다(실앱의 `retain` 은 연결 없이 먼저 알려 끊김 → 다시 붙기를 지난다 — 머리 주석).
    expect(documents.reconnect).not.toHaveBeenCalled();
    expect(watch).toHaveBeenCalledTimes(1);

    store.setState({ remote: { [MINI]: { client, status: RECONNECTING } } });
    expect(documents.disconnect).toHaveBeenCalledTimes(1);
    expect(documents.disconnect.mock.calls[0]![0]).toBe(MINI);
    // 같은 연결이라 감시는 그대로다.
    expect(watch).toHaveBeenCalledTimes(1);

    store.setState({ remote: { [MINI]: { client, status: CONNECTED } } });
    expect(documents.reconnect).toHaveBeenCalledExactlyOnceWith(MINI);
    stop();
  });

  it("matches only the dropped machine's documents", () => {
    const { hosts, client, store } = fakeHosts();
    const documents = fakeDocuments();
    startMachineDocumentSync(
      hosts,
      documents,
      vi.fn(() => () => {}),
    );
    store.setState({ remote: { [MINI]: { client, status: RECONNECTING } } });
    const match = documents.disconnect.mock.calls[0]![1] as (p: string, w?: string) => boolean;
    expect(match(`pitwall-remote://${encodeURIComponent(MINI)}/work/a.ts`, undefined)).toBe(true);
    expect(match("/work/a.ts", undefined)).toBe(false);
    expect(match("pitwall-remote://other/work/a.ts", undefined)).toBe(false);
  });
});
