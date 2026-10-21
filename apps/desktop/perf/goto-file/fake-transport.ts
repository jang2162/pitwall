// 측정 하네스의 가짜 Host(goto-file-bench.tsx 머리 주석). src/tauri-transport.ts 와 같은 꼴(`connectHost`)로, 프로젝트 창이 뜨는 데
// 드는 메서드와 `search.files` 에 메모리에서 답한다. 메시지는 IPC 처럼 JSON 문자열로 오간다(memory transport).

import {
  createMemoryTransportPair,
  type SearchFiles,
  type SearchFilesParams,
} from "@pitwall/rpc-client";

import { DEFAULT_KEYMAP_TOML } from "../../src/keymap/default-keymap";
import { i18nState } from "../../src/i18n/testing";

export type HostTarget = { hostId: string } | { projectId: string };

export const ROOT = "/work/demo";

/** 질의마다 결과 `limit` 개. 경로 · 일치 위치의 꼴은 합성 저장소(crates/pw-fs/examples/synth_repo.rs)의 결과와 비슷하게. */
export function answer(params: SearchFilesParams): SearchFiles {
  const word = params.query.replace(/\s+/g, "_");
  const files = Array.from({ length: params.limit ?? 100 }, (_, i) => {
    const relative = `apps/pkg-${i % 37}/src/module-${i}/${word}_${i}.rs`;
    const nameAt = relative.lastIndexOf("/") + 1;
    return {
      path: `${ROOT}/${relative}`,
      root: ROOT,
      relative,
      positions: [...Array(Math.min(word.length, 12)).keys()].map((k) => nameAt + k),
      score: 1000 - i,
      vendored: false,
      generated: false,
    };
  });
  return { files, matched: 100_000, indexed: 100_000 };
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
  const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
  server.onMessage((text) => {
    const m = JSON.parse(text) as { id?: number; method: string; params: Record<string, unknown> };
    const reply = (result: unknown) => send({ id: m.id, result });
    const path = (m.params?.path as string | undefined) ?? "";
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
      case "project.list":
        return reply([project]);
      case "project.trust":
        return reply({ projectId: "p1", ownerId: "p1", state: "trusted" });
      case "project.storage":
        return reply({ kind: "ok", path: "/data/state.db" });
      case "project.claimTrustPrompt":
        return reply(false);
      case "fs.list":
        return reply({
          path,
          entries:
            path === ROOT
              ? [
                  {
                    name: "src",
                    kind: "dir",
                    target: null,
                    outside: false,
                    size: 0,
                    mtimeMs: 1,
                    ignored: false,
                  },
                ]
              : [],
        });
      case "events.subscribe":
        return reply({ subscriptionId: next++ });
      case "events.unsubscribe":
        return reply(null);
      case "search.files":
        return reply(answer(m.params as unknown as SearchFilesParams));
      default:
        return m.id === undefined
          ? undefined
          : send({ id: m.id, error: { code: -32601, message: `no ${m.method}` } });
    }
  });
  return { transport: client, close: async () => {} };
}
