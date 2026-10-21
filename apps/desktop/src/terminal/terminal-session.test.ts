import { describe, expect, it } from "vitest";

import { FakeTerminalHost, settle } from "./fake-terminal-host";
import {
  HIGH_WATER,
  LOW_WATER,
  type SessionState,
  TerminalSession,
  type TerminalScreen,
} from "./terminal-session";

/** 그린 것을 모으는 화면. `manual` 이면 시험이 `render` 를 부를 때까지 그리기 콜백을 미룬다. */
function screen({ manual = false } = {}) {
  const chunks: (Uint8Array | string)[] = [];
  const pending: (() => void)[] = [];
  const s: TerminalScreen & { chunks: typeof chunks; text(): string; render(): void } = {
    chunks,
    write(data, callback) {
      chunks.push(data);
      if (!callback) return;
      if (manual) pending.push(callback);
      else callback();
    },
    text() {
      const decoder = new TextDecoder();
      return chunks
        .map((c) => (typeof c === "string" ? c : decoder.decode(c, { stream: true })))
        .join("");
    },
    render() {
      for (const cb of pending.splice(0)) cb();
    },
  };
  return s;
}

function start(host: FakeTerminalHost, view = screen(), worktree?: string) {
  const states: SessionState[] = [];
  const session = new TerminalSession({
    client: host.client,
    projectId: "p1",
    worktree,
    cols: 80,
    rows: 24,
    screen: view,
    onState: (s) => states.push(s),
    describeEnd: (s) => (s.kind === "exited" ? `EXIT ${s.code ?? "signal"}` : `FAIL ${s.message}`),
  });
  return { session, states, view };
}

describe("TerminalSession", () => {
  it("opens the stream with the window's worktree and writes output bytes in order until exited", async () => {
    const host = new FakeTerminalHost();
    const { states, view, session } = start(host, screen(), "/wt");
    expect(host.creates).toEqual([{ projectId: "p1", worktree: "/wt", cols: 80, rows: 24 }]);
    host.started("/wt/project");
    // 「가」(EA B0 80)를 청크 경계에서 자른다 — 바이트는 그대로 이어져 한 글자가 된다.
    host.output(new Uint8Array([0x61, 0xea]));
    host.output(new Uint8Array([0xb0, 0x80, 0x62]));
    host.push({ kind: "exited", code: 3 });
    host.push("end");
    await session.finished;
    expect(view.text()).toContain("a가b");
    expect(view.text()).toContain("EXIT 3");
    expect(states.map((s) => s.kind)).toEqual(["running", "exited"]);
    expect(states[0]).toMatchObject({ terminalId: 7, cwd: "/wt/project" });
  });

  it("keeps keys typed before the shell starts and sends them once it does", async () => {
    const host = new FakeTerminalHost();
    const { session } = start(host);
    session.input("ls");
    session.input(" -l\r");
    await settle();
    expect(host.inputs()).toEqual([]);
    host.started();
    await settle();
    expect(host.calls).toContainEqual({
      method: "terminal.input",
      params: { terminalId: 7, data: "ls -l\r" },
    });
  });

  it("sends one input at a time and batches what was typed meanwhile, in order", async () => {
    const host = new FakeTerminalHost();
    host.hold.add("terminal.input");
    const { session } = start(host);
    host.started();
    await settle();
    session.input("a");
    session.input("b");
    session.input("c");
    await settle();
    expect(host.inputs()).toEqual(["a"]);
    host.releaseHeld();
    await settle();
    expect(host.inputs()).toEqual(["a", "bc"]);
    host.releaseHeld();
    await settle();
    expect(host.inputs()).toEqual(["a", "bc"]);
  });

  it("sends only the latest size", async () => {
    const host = new FakeTerminalHost();
    host.hold.add("terminal.resize");
    const { session } = start(host);
    host.started();
    await settle();
    session.resize(100, 30);
    session.resize(110, 31);
    session.resize(120, 40);
    host.releaseHeld();
    await settle();
    host.releaseHeld();
    await settle();
    const sizes = host.calls.filter((c) => c.method === "terminal.resize").map((c) => c.params);
    expect(sizes).toEqual([
      { terminalId: 7, cols: 100, rows: 30 },
      { terminalId: 7, cols: 120, rows: 40 },
    ]);
  });

  it("stops pulling from the stream while too much output is not drawn yet", async () => {
    const host = new FakeTerminalHost();
    const view = screen({ manual: true });
    const { session } = start(host, view);
    host.started();
    const chunk = "x".repeat(16 * 1024);
    const chunks = Math.ceil(HIGH_WATER / chunk.length) + 8;
    for (let i = 0; i < chunks; i++) host.output(chunk);
    await settle();
    // started + HIGH_WATER 를 막 넘긴 청크까지 꺼내고 멈춘다.
    const pulledWhilePaused = host.pulled;
    expect(pulledWhilePaused).toBe(1 + Math.floor(HIGH_WATER / chunk.length) + 1);
    await settle(10);
    expect(host.pulled).toBe(pulledWhilePaused);
    // 그리면(LOW_WATER 아래로) 다시 꺼낸다.
    view.render();
    await settle();
    expect(host.pulled).toBeGreaterThan(pulledWhilePaused);
    expect(LOW_WATER).toBeLessThan(HIGH_WATER);
    session.dispose();
  });

  it("reports a stream that fails before starting and drops later input", async () => {
    const host = new FakeTerminalHost();
    const { session, states, view } = start(host);
    host.push(new Error("cwd is not inside project"));
    await session.finished;
    expect(states).toEqual([{ kind: "failed", message: "cwd is not inside project" }]);
    expect(view.text()).toContain("FAIL cwd is not inside project");
    session.input("x");
    await settle();
    expect(host.inputs()).toEqual([]);
  });

  it("cancels the stream on dispose without reporting an end", async () => {
    const host = new FakeTerminalHost();
    const { session, states } = start(host);
    host.started();
    await settle();
    session.dispose();
    await session.finished;
    expect(host.cancelled).toBe(true);
    expect(states.map((s) => s.kind)).toEqual(["running"]);
  });
});
