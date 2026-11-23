// 원격 탭의 언어 서버(18-m7-plan M7-21 ⑥ — M7-13 이 넘김). 다른 머신의 탭 문서는 그 머신의 언어 서버로 열고(그 머신의 프로젝트 id ·
// 접두 없는 그 머신의 경로), 기능 행의 RPC · 진단 이벤트도 그 머신으로 간다. 이 머신 서버와 원격 서버에 **같은 경로**의 문서를 두어
// 엉뚱한 머신으로 새면 드러나게 했다(공격자 입장: 다른 머신의 경로가 이 머신 Host 의 같은 경로 파일로 읽히는 길).

import type { EventMessage, RpcClient } from "@pitwall/rpc-client";
import { createStore } from "zustand/vanilla";
import { describe, expect, it } from "vitest";

import { checkoutKey, qualify } from "../checkout/checkout";
import { createLspSessions, type LspRoute } from "./lsp-sessions";
import { fakeLspDocuments, fakeLspServer } from "./lsp-testing";
import { machineLspRoutes, type MachineHosts } from "./machine-routes";
import { documentSource, targetOf, usageFiles } from "./navigation";

const MINI = "SHA256:mini/fingerprint";
const PATH = "/work/demo/src/a.rs";
const REMOTE_PATH = qualify(MINI, PATH);
const settle = () => new Promise((r) => setTimeout(r, 0));

function setup(routes: Record<string, LspRoute> = {}) {
  const here = fakeLspServer();
  const mini = fakeLspServer();
  const documents = fakeLspDocuments({ [PATH]: "fn here() {}", [REMOTE_PATH]: "fn mini() {}" });
  const sessions = createLspSessions({
    source: here.source,
    services: here.services,
    documents,
    projectId: "p1",
    machine: (machine) =>
      routes[machine] ??
      (machine === MINI
        ? { projectId: "r1", source: mini.source, services: mini.services }
        : undefined),
  });
  return { here, mini, sessions };
}

describe("a document of another machine's tab", () => {
  it("opens on that machine's language server with that machine's project and path", async () => {
    const { here, mini, sessions } = setup();
    const worktree = checkoutKey({ machine: MINI, worktree: "/work/demo-wt" });
    const handle = sessions.acquire(REMOTE_PATH, "Rust", worktree);
    await settle();
    expect(mini.called("open")).toEqual([
      {
        projectId: "r1",
        worktree: "/work/demo-wt",
        path: PATH,
        language: "Rust",
        version: 0,
        text: "fn mini() {}",
      },
    ]);
    expect(here.called("open")).toEqual([]);
    // 기능 행이 보는 문서는 그 머신의 꼴이다 — 싣는 인자와 Host 가 돌려준 경로가 같은 꼴.
    const doc = handle.document;
    expect([doc.projectId, doc.path, doc.worktree, doc.machine]).toEqual([
      "r1",
      PATH,
      "/work/demo-wt",
      MINI,
    ]);
    // 기능 행의 RPC 는 그 머신으로.
    mini.handle("lsp.hover", async () => ({ contents: "mini", range: null, staleVersion: false }));
    here.handle("lsp.hover", async () => ({ contents: "here", range: null, staleVersion: false }));
    const hover = await doc.call("hover", "lsp.hover", (at) => ({
      docId: at.docId,
      version: at.version,
      offset: 1,
    }));
    expect((hover as unknown as { contents: string } | undefined)?.contents).toBe("mini");
    expect(here.called("call:lsp.hover")).toEqual([]);
    // 진단 이벤트도 그 머신의 구독으로 듣는다.
    const heard: EventMessage[] = [];
    doc.services.subscribe(["lsp.diagnostics"], (e) => heard.push(e));
    here.emit("lsp.diagnostics", { path: PATH });
    mini.emit("lsp.diagnostics", { path: PATH });
    expect(heard).toHaveLength(1);
    handle.release();
    await settle();
    expect(mini.called("close")).toEqual([{ docId: "d1" }]);
  });

  it("keeps this machine's documents on this machine", async () => {
    const { here, mini, sessions } = setup();
    const handle = sessions.acquire(PATH, "Rust");
    await settle();
    expect(here.called("open")).toHaveLength(1);
    expect(mini.called("open")).toEqual([]);
    expect(handle.document.machine).toBe("local");
    handle.release();
  });

  it("does not open a document of a machine the window has no route to — not even on this machine", async () => {
    const { here, mini, sessions } = setup();
    const other = qualify("SHA256:other/x", PATH);
    const handle = sessions.acquire(other, "Rust", checkoutKey({ machine: "SHA256:other/x" }));
    await settle();
    expect(here.called("open")).toEqual([]);
    expect(mini.called("open")).toEqual([]);
    expect(handle.document.capabilities).toEqual({});
    // 기능 행이 이 머신에 싣게 되는 경로는 그 머신 경로가 아니다(이 머신의 같은 경로 파일로 새지 않게).
    expect(handle.document.path).toBe(other);
    handle.release();
  });

  it("opens the locations its server returned on that machine", async () => {
    const { sessions } = setup();
    const handle = sessions.acquire(REMOTE_PATH, "Rust", checkoutKey({ machine: MINI }));
    await settle();
    const source = documentSource(handle.document);
    const loc = {
      path: "/work/demo/src/b.rs",
      line: 3,
      column: 1,
      endLine: 3,
      endColumn: 4,
      token: null,
      preview: "fn b",
    };
    expect(targetOf(loc as never, source)).toMatchObject({ path: loc.path, machine: MINI });
    expect(usageFiles([loc as never], source)[0]).toMatchObject({ path: loc.path, machine: MINI });
    handle.release();
  });
});

describe("machineLspRoutes", () => {
  function hosts() {
    const store = createStore(() => ({ seq: 0 }));
    const clients = new Map<string, RpcClient>();
    const members = new Map([[MINI, "r1"]]);
    const sent: { machine: string; method: string; params: unknown }[] = [];
    const subscribed: { machine: string; projectId: string }[] = [];
    const clientOf = (machine: string) =>
      ({
        call: async (method: string, params: unknown) => {
          sent.push({ machine, method, params });
          return { docId: "x", off: null, instances: [] };
        },
        subscribe: async (params: { projectId: string }) => {
          subscribed.push({ machine, projectId: params.projectId });
          return { unsubscribe: async () => {} };
        },
      }) as unknown as RpcClient;
    const h: MachineHosts = {
      store,
      connection: (machine) => ({ client: clients.get(machine), projectId: members.get(machine) }),
      client: async (machine) => ({
        client: clients.get(machine)!,
        projectId: members.get(machine)!,
      }),
    };
    return {
      h,
      sent,
      subscribed,
      connect(machine: string) {
        clients.set(machine, clientOf(machine));
        store.setState({ seq: store.getState().seq + 1 });
      },
    };
  }

  it("sends a machine's document calls on that machine's connection and listens there once it connects", async () => {
    const fx = hosts();
    const routes = machineLspRoutes(() => fx.h, {});
    expect(routes("SHA256:not/member")).toBeUndefined();
    const route = routes(MINI)!;
    expect(route.projectId).toBe("r1");
    expect(routes(MINI)).toBe(route);
    route.services.subscribe(["lsp.diagnostics"], () => {});
    expect(fx.subscribed).toEqual([]);
    fx.connect(MINI);
    await settle();
    expect(fx.subscribed).toEqual([{ machine: MINI, projectId: "r1" }]);
    await route.source.open({
      projectId: "r1",
      path: PATH,
      language: "Rust",
      version: 0,
      text: "",
    });
    expect(fx.sent.map((s) => [s.machine, s.method])).toEqual([[MINI, "lsp.document.open"]]);
  });
});
