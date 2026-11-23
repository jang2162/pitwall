// 커밋 보내기의 흐름(18-m7-plan M7-17): Desktop 이 두 머신 연결 사이를 잇는다. 가짜 Host 는 실제 범위 규칙대로 거절한다
// (fake-send-host.ts). 머신 id 의 `sourceMachine` 꼴, 고르는 팝업의 기본값(마지막 대상), [거기서 …] 단추도 여기서 본다.

import { ActionRegistry } from "@pitwall/keymap";
import { createMemoryTransportPair, type ProjectBundle, RpcClient } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import {
  commit,
  createGitHost,
  createRepo,
  type FakeGitHost,
  type FakeScope,
} from "./fake-send-host";
import { preselected, RUN_IN_ACTION, TERMINAL_IN_ACTION, thereButtons } from "./send-feature";
import { sendCommits, sourceMachineName } from "./send-flow";
import { gitSendStrings } from "./strings";

const MINI = "SHA256:mini+/fingerprint=";

/** 가짜 Host 에 붙은 클라이언트. `log` 는 서버가 받은 메서드 순서. */
function connect(host: FakeGitHost, scope: FakeScope, log: string[]) {
  const [client, server] = createMemoryTransportPair();
  let next = 1;
  const conn = {
    scope,
    send: (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m })),
  };
  server.onMessage((text) => {
    const m = JSON.parse(text) as { id?: number; method: string; params?: Record<string, unknown> };
    if (m.method.startsWith("stream.")) return;
    log.push(`${host.repo.machine}:${m.method}`);
    if (!host.handle(conn, m.id, m.method, m.params ?? {}, () => next++))
      conn.send({ id: m.id, error: { code: -32601, message: `no ${m.method}` } });
  });
  return new RpcClient(client);
}

function machines() {
  const local = createGitHost(createRepo("local", "p1", "/work/demo"));
  const mini = createGitHost(createRepo(MINI, "r1", "/work/demo"));
  local.repo.branches.set("feat", "c0");
  commit(local.repo, "feat", "c1");
  commit(local.repo, "feat", "c2");
  local.repo.head = "feat";
  return { local, mini };
}

describe("sendCommits", () => {
  it("reads theirs on the target, streams the source bundle into the target and applies it there", async () => {
    const { local, mini } = machines();
    const log: string[] = [];
    const source = connect(local, { kind: "project", projectId: "p1" }, log);
    const target = connect(mini, { kind: "project", projectId: "r1" }, log);
    // 대상의 git.prompts 를 듣는 창이 있다(경로가 바뀌는 apply 의 미저장 확인).
    void (async () => {
      for await (const e of target.stream("git.prompts", { projectId: "r1" })) {
        if (e.kind === "open")
          await target.call("git.prompt.answer", {
            projectId: "r1",
            promptId: e.prompt.promptId,
            answer: { kind: "proceed" },
          });
      }
    })();
    const progress: [number, number][] = [];
    const result = await sendCommits({
      source: { client: source, projectId: "p1", machine: "local", worktree: null },
      target: { client: target, projectId: "r1", machine: MINI, worktree: null },
      branch: "feat",
      commit: "c2",
      onProgress: (sent, size) => progress.push([sent, size]),
    });
    expect(result.outcome).toMatchObject({
      kind: "applied",
      previous: null,
      current: "c2",
      created: true,
    });
    expect(result.incomingRef).toBe("refs/pitwall/incoming/local/feat");
    expect(mini.repo.branches.get("feat")).toBe("c2");
    expect(mini.repo.head).toBe("feat");
    // 원본에는 읽기(prepare)만, 대상에는 받기 · 맞추기만 간다.
    const at = (machine: string) => log.filter((l) => l.startsWith(`${machine}:`));
    expect(at("local").filter((l) => !l.endsWith("git.prompts"))).toEqual([
      "local:git.send.prepare",
    ]);
    expect(at(MINI).filter((l) => l.includes("git.send."))[0]).toBe(`${MINI}:git.send.theirs`);
    expect(
      at(MINI)
        .filter((l) => l.includes("git.send."))
        .at(-1),
    ).toBe(`${MINI}:git.send.apply`);
    // 대상이 가진 커밋(c0)은 빠진다 — theirs 를 prepare 에 넘겼다.
    const prepare = local.repo.calls.find((c) => c.method === "git.send.prepare")!;
    expect(prepare.params.theirs).toEqual(["c0"]);
    // 청크는 여럿이고 순서대로, 마지막은 전체 크기.
    expect(progress.length).toBeGreaterThan(2);
    expect(progress.at(-1)![0]).toBe(progress.at(-1)![1]);
  });

  it("is refused by a host when the target side is not that project's window connection", async () => {
    const { local, mini } = machines();
    const log: string[] = [];
    const source = connect(local, { kind: "project", projectId: "p1" }, log);
    // 대상 머신의 Host 범위 연결(웰컴 · 설정 창 · 링크) — 쓰기를 받지 않는다.
    const target = connect(mini, { kind: "host" }, log);
    await expect(
      sendCommits({
        source: { client: source, projectId: "p1", machine: "local", worktree: null },
        target: { client: target, projectId: "r1", machine: MINI, worktree: null },
        branch: "feat",
        commit: "c2",
      }),
    ).rejects.toMatchObject({ code: -32001 });
    expect(mini.repo.branches.has("feat")).toBe(false);
  });

  it("names the incoming ref after the source machine in the engine's name rule when receiving from it", async () => {
    const { local, mini } = machines();
    mini.repo.branches.set("fix", "c0");
    commit(mini.repo, "fix", "m1");
    const log: string[] = [];
    const target = connect(local, { kind: "project", projectId: "p1" }, log);
    void (async () => {
      for await (const e of target.stream("git.prompts", { projectId: "p1" }))
        if (e.kind === "open")
          await target.call("git.prompt.answer", {
            projectId: "p1",
            promptId: e.prompt.promptId,
            answer: { kind: "proceed" },
          });
    })();
    const result = await sendCommits({
      source: {
        client: connect(mini, { kind: "project", projectId: "r1" }, log),
        projectId: "r1",
        machine: MINI,
        worktree: null,
      },
      target: { client: target, projectId: "p1", machine: "local", worktree: null },
      branch: "fix",
      commit: "m1",
    });
    expect(result.incomingRef).toBe("refs/pitwall/incoming/sha256-mini-_fingerprint/fix");
    expect(local.repo.branches.get("fix")).toBe("m1");
  });

  it("stops without touching a diverged target branch", async () => {
    const { local, mini } = machines();
    commit(mini.repo, "feat", "x9");
    const log: string[] = [];
    const result = await sendCommits({
      source: {
        client: connect(local, { kind: "project", projectId: "p1" }, log),
        projectId: "p1",
        machine: "local",
        worktree: null,
      },
      target: {
        client: connect(mini, { kind: "project", projectId: "r1" }, log),
        projectId: "r1",
        machine: MINI,
        worktree: null,
      },
      branch: "feat",
      commit: "c2",
    });
    expect(result.outcome).toMatchObject({ kind: "stopped", reason: "diverged" });
    expect(mini.repo.branches.get("feat")).toBe("x9");
  });
});

describe("sourceMachineName", () => {
  it("turns a machine id into the engine's name rule", () => {
    expect(sourceMachineName("local")).toBe("local");
    const name = sourceMachineName(MINI);
    expect(name).toBe("sha256-mini-_fingerprint");
    expect(name).toMatch(/^[A-Za-z0-9._-]{1,128}$/);
    expect(sourceMachineName(".x")).toBe("m.x");
    expect(sourceMachineName("a.lock")).toBe("a_lock");
    expect(sourceMachineName("x".repeat(200))).toHaveLength(128);
  });
});

const bundle = (lastTarget: ProjectBundle["members"][number]["lastTarget"]): ProjectBundle => ({
  id: "b-1",
  name: "demo",
  favorite: false,
  tags: [],
  openedAt: null,
  implicit: false,
  members: [
    { machine: "local", projectId: "p1", lastTarget },
    { machine: MINI, projectId: "r1", lastTarget: { machine: "local", worktree: "/work/wt" } },
  ],
});

describe("preselected", () => {
  it("picks the source member's last target when sending", () => {
    expect(
      preselected(bundle({ machine: MINI, worktree: "/w/x" }), "send", { machine: "local" }),
    ).toEqual({
      machine: MINI,
      worktree: "/w/x",
    });
    expect(preselected(bundle(null), "send", { machine: "local" })).toBeUndefined();
  });

  it("picks the member whose last target is the active checkout when receiving", () => {
    const b = bundle(null);
    expect(preselected(b, "receive", { machine: "local", worktree: "/work/wt" })).toEqual({
      machine: MINI,
    });
    expect(preselected(b, "receive", { machine: "local" })).toBeUndefined();
  });
});

describe("thereButtons", () => {
  const T = gitSendStrings((id) => id);
  const checkout = { machine: MINI, worktree: null };

  it("adds a button only for an action that takes the checkout argument", () => {
    const registry = new ActionRegistry();
    const calls: unknown[] = [];
    registry.register({
      id: RUN_IN_ACTION,
      label: "run in",
      args: true,
      run: (a) => void calls.push(a),
    });
    // 인자를 모르는 액션 — 부르면 활성 체크아웃에서 연다. 단추를 달지 않는다.
    registry.register({
      id: TERMINAL_IN_ACTION,
      label: "terminal",
      run: () => void calls.push("active"),
    });
    const buttons = thereButtons(registry, checkout, T);
    expect(buttons).toHaveLength(1);
    buttons[0]!.run();
    expect(calls).toEqual([{ checkout }]);
  });

  it("adds nothing when the actions are missing", () => {
    expect(thereButtons(new ActionRegistry(), checkout, T)).toEqual([]);
  });
});
