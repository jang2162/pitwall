// Git 로그 측정 하네스의 가짜 Host(git-log-bench.tsx 머리 주석). src/tauri-transport.ts 와 같은 꼴(`connectHost`)로, 프로젝트 창이
// 뜨는 데 드는 메서드와 `git.log` 에 메모리에서 답한다. `git.log` 는 pw-git 이 합성 저장소에서 낸 스트림 그대로(`log_timing --dump`
// 의 청크 배열, `window.__gitLogChunks`)를 청크마다 보낸다 — 메시지는 IPC 처럼 JSON 문자열로 오간다(memory transport).
//
// Git 시나리오 스모크(scenario-bench.tsx)를 위해 둘을 더 흉내 낸다: `window.__gitShowFixture` 가 있으면 `git.show` · `git.diff` ·
// `git.blob` 이 그 파일 하나짜리 커밋으로 답하고, `window.__fakeCheckout(이름)` 은 HEAD 이름을 바꾸고 `git.refsChanged` 를 보낸다.

import { createMemoryTransportPair, type LogChunk } from "@pitwall/rpc-client";

import { DEFAULT_KEYMAP_TOML } from "../../src/keymap/default-keymap";
import { i18nState } from "../../src/i18n/testing";

export type HostTarget = { hostId: string } | { projectId: string };

export const ROOT = "/work/demo";

declare global {
  interface Window {
    __gitLogChunks?: LogChunk[];
    /** perf/diff/ten-thousand.ts 의 `input()` — `git.diff` 와 옛 · 새 `git.blob` 의 JSON. */
    __gitShowFixture?: { diff: string; old: string; new: string };
    __fakeCheckout?: (branch: string) => void;
  }
}

const project = {
  id: "p1",
  name: "demo",
  host: "local",
  roots: [ROOT],
  favorite: false,
  tags: [],
  configMode: "repository",
  slug: "demo",
  createdAt: 1,
  openedAt: null,
};

export async function connectHost(_target: HostTarget) {
  const [client, server] = createMemoryTransportPair();
  let next = 1;
  let head = "main";
  const subscriptions = new Map<number, string[]>();
  const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
  window.__fakeCheckout = (branch) => {
    head = branch;
    for (const [subscriptionId, topics] of subscriptions)
      if (topics.includes("git.refsChanged"))
        send({
          method: "events.event",
          params: {
            subscriptionId,
            projectId: "p1",
            topic: "git.refsChanged",
            payload: { worktree: null },
          },
        });
  };
  const blob = (json: string) => ({
    ...JSON.parse(json),
    id: "0".repeat(40),
    mode: "100644",
    size: json.length,
    tooLarge: false,
    offset: 0,
    eof: true,
  });
  server.onMessage((text) => {
    const m = JSON.parse(text) as { id?: number; method: string; params: Record<string, unknown> };
    const reply = (result: unknown) => send({ id: m.id, result });
    switch (m.method) {
      case "i18n.get":
        return reply(i18nState("ko"));
      case "config.get":
        return reply({ snapshot: { files: [], notices: [] }, error: null });
      case "keymap.get":
        return reply({
          id: "pitwall-default",
          extension: "dev.pitwall.keymap-default",
          kind: "base",
          content: DEFAULT_KEYMAP_TOML,
        });
      case "host.info":
        return reply({ hostId: "local", kind: "local", version: "0", os: "t", arch: "t" });
      case "task.list":
        return reply([]);
      case "project.get":
        return reply(project);
      case "project.trust":
        return reply({ projectId: "p1", ownerId: "p1", state: "trusted" });
      case "project.storage":
        return reply({ kind: "ok", path: "/data/state.db" });
      case "project.claimTrustPrompt":
        return reply(false);
      case "fs.list":
        return reply({ path: m.params?.path, entries: [] });
      case "git.status":
        return reply({ workTree: ROOT, entries: [], unsupported: null });
      case "git.refs":
        return reply({
          workTree: ROOT,
          head: { kind: "branch", name: head, commit: "0".repeat(40) },
          branches: [],
          remoteBranches: [],
          tags: [],
          unsupported: null,
          broken: [],
        });
      case "git.operation":
        return reply({ operation: null });
      case "git.watch": {
        const streamId = next++;
        reply({ streamId });
        return send({ method: "stream.chunk", params: { streamId, data: { workTree: ROOT } } });
      }
      case "git.log": {
        const streamId = next++;
        reply({ streamId });
        const chunks = window.__gitLogChunks ?? [];
        // 청크 사이에 한 번씩 양보한다 — Host 의 청크도 따로 도착한다.
        void (async () => {
          for (const data of chunks) {
            send({ method: "stream.chunk", params: { streamId, data } });
            await new Promise((r) => setTimeout(r, 0));
          }
          send({ method: "stream.end", params: { streamId, status: "completed" } });
        })();
        return;
      }
      case "git.show": {
        const rev = String(m.params.rev);
        return reply({
          commit: rev,
          tree: "1".repeat(40),
          parents: ["a"],
          shallow: false,
          author: { name: "Pitwall", email: "p@pitwall.invalid", time: 1_700_000_000, offset: 0 },
          committer: {
            name: "Pitwall",
            email: "p@pitwall.invalid",
            time: 1_700_000_000,
            offset: 0,
          },
          message: "perf: change 10,000 lines\n",
          head: true,
          refs: [],
        });
      }
      case "git.diff":
        return window.__gitShowFixture
          ? reply({ ...JSON.parse(window.__gitShowFixture.diff), workTree: ROOT })
          : send({ id: m.id, error: { code: -32601, message: "no fixture" } });
      case "git.blob": {
        const fixture = window.__gitShowFixture;
        if (!fixture) return send({ id: m.id, error: { code: -32601, message: "no fixture" } });
        return reply(blob(m.params.rev === "a" ? fixture.old : fixture.new));
      }
      case "events.subscribe": {
        const subscriptionId = next++;
        subscriptions.set(subscriptionId, (m.params.topics as string[] | undefined) ?? []);
        return reply({ subscriptionId });
      }
      case "events.unsubscribe":
        return reply(null);
      default:
        return m.id === undefined
          ? undefined
          : send({ id: m.id, error: { code: -32601, message: `no ${m.method}` } });
    }
  });
  return { transport: client, close: async () => {} };
}
