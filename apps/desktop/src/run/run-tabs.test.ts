// @vitest-environment happy-dom
// 창의 실행 탭(run-tabs.ts): 상대 경로 링크가 그 실행의 작업 폴더로 풀린다(완료 기준 ①), 에이전트가 띄운 실행의 탭(④), 다시 실행의
// 탭 이어받기, 목록으로 되살린 탭이 `since: 0` 으로 앞 출력을 받는다(③의 모델 쪽), 프로그램 출력의 OSC 52 · 8 · 2 가 사람의 클립보드 ·
// 브라우저 · 창 제목에 닿지 않는다.

import type { IBufferRange, ILink } from "@xterm/xterm";
import { RpcCallError } from "@pitwall/rpc-client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { oscLinkHandler } from "../terminal/xterm-view";
import { FakeRunHost, runInfo, settle } from "./fake-run-host";
import { type RunDeps, RunTabs, TOPIC_EXITED, TOPIC_STARTED } from "./run-tabs";

let host: FakeRunHost;
let tabs: RunTabs;
const opened: unknown[] = [];
const urls: string[] = [];

function deps(): RunDeps {
  return {
    client: host.client,
    projectId: "p1",
    takesKey: () => false,
    opensLink: (e) => e.metaKey,
    openFile: (target) => opened.push(target),
    openUrl: (url) => urls.push(url),
    linkHint: (url, differs) => (differs ? `differs ${url}` : `opens ${url}`),
    exitedLine: (exit) => `[exit ${exit.code}]`,
    droppedLine: (bytes) => `[dropped ${bytes}]`,
    failedLine: (message) => `[failed ${message}]`,
  };
}

beforeEach(() => {
  host = new FakeRunHost();
  tabs = new RunTabs();
  tabs.setDeps(deps());
  opened.length = 0;
  urls.length = 0;
  sessionStorage.clear();
  vi.spyOn(console, "warn").mockImplementation(() => {});
});

afterEach(() => {
  tabs.disposeAll();
  vi.restoreAllMocks();
});

function write(key: string, text: string): Promise<void> {
  return new Promise((r) => tabs.console(key)!.term.write(text, r));
}

/** 줄 `y`(1부터)의 `파일:줄` 링크. */
function linksAt(key: string, y: number): Promise<ILink[]> {
  return new Promise((r) => tabs.console(key)!.view.links.provideLinks(y, (l) => r(l ?? [])));
}

const click = (meta = true) => new MouseEvent("click", { metaKey: meta });
const RANGE: IBufferRange = { start: { x: 1, y: 1 }, end: { x: 1, y: 1 } };

describe("file links in a run console", () => {
  it("resolve relative paths against the run's working directory", async () => {
    // 프로젝트 루트는 /main, 실행의 작업 폴더는 /main/app. 셸 터미널처럼 루트 기준으로 풀면 /main/src/app.ts — 없는 파일이다.
    const stats: unknown[] = [];
    host.results["fs.stat"] = (params) => {
      stats.push(params);
      return params.path === "/main/app/src/app.ts" ? { kind: "file" } : { kind: "missing" };
    };
    tabs.upsert(runInfo({ runId: "r1", cwd: "/main/app", worktree: "/wt" }));
    await write("r1", "  at src/app.ts:12:5\r\n");
    const links = await linksAt("r1", 1);
    expect(links.map((l) => l.text)).toEqual(["src/app.ts:12:5"]);
    links[0]!.activate(click(), links[0]!.text);
    expect(opened).toEqual([
      { path: "/main/app/src/app.ts", line: 12, column: 5, worktree: "/wt" },
    ]);
    expect(stats).toContainEqual({
      projectId: "p1",
      worktree: "/wt",
      path: "/main/app/src/app.ts",
    });
  });

  it("open only with ⌘ · Ctrl", async () => {
    host.results["fs.stat"] = () => ({ kind: "file" });
    tabs.upsert(runInfo({ runId: "r1" }));
    await write("r1", "see a/b.ts:3\r\n");
    const [link] = await linksAt("r1", 1);
    link!.activate(click(false), link!.text);
    expect(opened).toEqual([]);
  });
});

describe("escape sequences from the program", () => {
  it("do not write the clipboard (OSC 52) or the window title (OSC 2)", async () => {
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    document.title = "Pitwall";
    tabs.upsert(runInfo({ runId: "r1" }));
    const term = tabs.console("r1")!.term;
    const container = document.createElement("div");
    document.body.append(container);
    tabs.console("r1")!.attach(container);
    await write("r1", `\x1b]52;c;${btoa("rm -rf ~")}\x07\x1b]2;owned\x07done`);
    expect(writeText).not.toHaveBeenCalled();
    expect(document.title).toBe("Pitwall");
    expect(term.buffer.active.getLine(0)?.translateToString(true)).toBe("done");
    container.remove();
  });

  it("open OSC 8 links only for http(s) and only with ⌘ · Ctrl", () => {
    const handler = oscLinkHandler(() => ({
      stat: async () => false,
      takesKey: () => false,
      opensLink: (e) => e.metaKey,
      openFile: () => {},
      openUrl: (url) => urls.push(url),
    }));
    handler.activate(click(), "javascript:alert(1)", RANGE);
    handler.activate(click(), "file:///etc/passwd", RANGE);
    handler.activate(click(false), "https://example.com/", RANGE);
    handler.activate(click(), "https://example.com/docs", RANGE);
    expect(urls).toEqual(["https://example.com/docs"]);
    expect(handler.allowNonHttpProtocols).toBe(false);
  });
});

describe("OSC 8 links in a run console", () => {
  /** 콘솔을 화면에 붙이고 `text` 를 쓴다. */
  async function shown(text: string) {
    tabs.upsert(runInfo({ runId: "r1" }));
    const container = document.createElement("div");
    document.body.append(container);
    tabs.console("r1")!.attach(container);
    await write("r1", text);
    return tabs.console("r1")!.term;
  }

  it("go through the console's own handler: ⌘ + http(s) only, and the real address on hover", async () => {
    const term = await shown("\x1b]8;;https://evil.example/x\x07docs\x1b]8;;\x07 and more");
    const handler = term.options.linkHandler;
    expect(handler).toBeTruthy();
    // 보이는 글 「docs」(1~4칸) 뒤의 주소는 다른 곳이다.
    const range: IBufferRange = { start: { x: 1, y: 1 }, end: { x: 4, y: 1 } };
    handler!.activate(click(false), "https://evil.example/x", range);
    expect(urls).toEqual([]);
    handler!.hover?.(click(false), "https://evil.example/x", range);
    expect(term.element!.title).toBe("differs https://evil.example/x");
    handler!.leave?.(click(false), "https://evil.example/x", range);
    expect(term.element!.hasAttribute("title")).toBe(false);
    handler!.activate(click(), "https://evil.example/x", range);
    expect(urls).toEqual(["https://evil.example/x"]);
  });

  it("say plainly where a link goes when its text is the address", async () => {
    const term = await shown("\x1b]8;;https://example.com/\x07example.com\x1b]8;;\x07");
    const range: IBufferRange = { start: { x: 1, y: 1 }, end: { x: 11, y: 1 } };
    term.options.linkHandler!.hover?.(click(false), "https://example.com/", range);
    expect(term.element!.title).toBe("opens https://example.com/");
  });
});

describe("run tabs", () => {
  it("restart a chain through its sql step's risk confirmation (M6-21)", async () => {
    // 다시 띄우는 구성이 사슬이고 `sql` 단계가 위험 확인(3001, `data.config`)을 청한다 — 그 확인을 받아 `sqlConfirmed` 로 다시 부른다.
    const next = runInfo({ runId: "B", restartOf: "A" });
    host.results["run.restart"] = (p) => {
      if (!p.sqlConfirmed)
        throw new RpcCallError({
          code: 3001,
          message: "confirm",
          data: {
            config: "seed",
            request: { statementsHash: "s1", grade: "write" },
            connection: { id: "lite", name: "lite", production: false, readOnly: false },
            files: [],
            transaction: "all",
            sql: "UPDATE t SET v = 1;",
          } as never,
        });
      return next;
    };
    const asked: string[] = [];
    tabs.setDeps({
      ...deps(),
      askSql: async (data) => {
        asked.push(data.request.statementsHash);
        return { statementsHash: data.request.statementsHash, grade: "write", name: null };
      },
    });
    tabs.upsert(runInfo({ runId: "A" }));
    await tabs.restart("A");
    expect(asked).toEqual(["s1"]);
    const sent = host.calls.filter((c) => c.method === "run.restart").map((c) => c.params);
    expect(sent.at(-1)).toMatchObject({
      runId: "A",
      sqlConfirmed: { seed: { statementsHash: "s1", grade: "write", name: null } },
    });
    expect(tabs.store.getState().tabs.map((t) => t.run.runId)).toEqual(["B"]);
  });

  it("keep a closed tab closed after a reload even when it was rerun", async () => {
    // 리뷰 재현: A → 다시 실행 B(restartOf A) → B 가 끝난 뒤 탭을 닫고 창을 다시 불러온다. 목록은 [A, B] 시작 순이다.
    const a = runInfo({ runId: "A", state: "exited", exit: { code: 0, signal: null } });
    const b = runInfo({ runId: "B", restartOf: "A" });
    host.results["run.restart"] = () => b;
    tabs.upsert(runInfo({ runId: "A" }));
    await tabs.restart("A");
    tabs.upsert({ ...b, state: "exited", exit: { code: 0, signal: null } });
    tabs.close("A");
    expect(tabs.store.getState().tabs).toEqual([]);
    host.runs = [a, { ...b, state: "exited", exit: { code: 0, signal: null } }];
    const again = new RunTabs();
    again.setDeps(deps());
    again.connect(host.client, "p1");
    await settle();
    expect(again.store.getState().tabs.map((t) => [t.key, t.run.runId])).toEqual([]);
    again.disposeAll();
  });

  it("open a tab for a run an agent started without taking the active tab", async () => {
    tabs.connect(host.client, "p1");
    await settle();
    host.event(TOPIC_STARTED, runInfo({ runId: "mine" }));
    host.event(
      TOPIC_STARTED,
      runInfo({
        runId: "agent",
        launchedBy: { kind: "agent", session: "s1", client: "claude-code" },
      }),
    );
    const { tabs: list, activeKey } = tabs.store.getState();
    expect(list.map((t) => [t.key, t.run.launchedBy.kind])).toEqual([
      ["mine", "window"],
      ["agent", "agent"],
    ]);
    expect(activeKey).toBe("mine");
    // 탭이 없을 때 온 에이전트 실행은 활성이 된다.
    const other = new RunTabs();
    other.setDeps(deps());
    other.upsert(
      runInfo({ runId: "a2", launchedBy: { kind: "agent", session: "s1", client: null } }),
    );
    expect(other.store.getState().activeKey).toBe("a2");
    other.disposeAll();
  });

  it("follow events and the list without doubling a run", async () => {
    host.runs = [runInfo({ runId: "r1" })];
    tabs.connect(host.client, "p1");
    host.event(
      TOPIC_EXITED,
      runInfo({ runId: "r1", state: "exited", exit: { code: 0, signal: null } }),
    );
    await settle();
    // 목록(실행 중)이 이벤트(끝남)보다 늦게 왔다 — 끝난 실행을 되살리지 않는다.
    const [tab, ...rest] = tabs.store.getState().tabs;
    expect(rest).toEqual([]);
    expect(tab!.run.state).toBe("exited");
  });

  it("keep the tab when a run is rerun, and drop the old run's late events", async () => {
    host.results["run.restart"] = () => runInfo({ runId: "r2", restartOf: "r1" });
    tabs.connect(host.client, "p1");
    await settle();
    host.event(TOPIC_STARTED, runInfo({ runId: "r1" }));
    await tabs.restart("r1");
    host.event(TOPIC_STARTED, runInfo({ runId: "r2", restartOf: "r1" }));
    host.event(TOPIC_EXITED, runInfo({ runId: "r1", state: "exited" }));
    const list = tabs.store.getState().tabs;
    expect(list.map((t) => [t.key, t.run.runId])).toEqual([["r1", "r2"]]);
    expect(host.calls).toContainEqual({
      method: "run.restart",
      params: { runId: "r1", confirmed: null, sqlConfirmed: null },
    });
  });

  it("restore a reloaded window's tabs from the list and replay their output from 0", async () => {
    const run = runInfo({ runId: "r1", logEnd: 12 });
    host.runs = [run];
    tabs.connect(host.client, "p1");
    await settle();
    expect(tabs.store.getState().tabs.map((t) => t.key)).toEqual(["r1"]);
    tabs.console("r1")!.start();
    host.attached(run);
    host.output("r1", 0, "before reload\r\n");
    await settle(20);
    expect(host.attaches).toEqual([{ runId: "r1", since: 0 }]);
    const term = tabs.console("r1")!.term;
    expect(term.buffer.active.getLine(0)?.translateToString(true)).toBe("before reload");
  });

  it("close only ended runs, and keep a closed one closed after a reload", async () => {
    tabs.upsert(runInfo({ runId: "live" }));
    tabs.close("live");
    expect(tabs.store.getState().tabs).toHaveLength(1);
    tabs.upsert(runInfo({ runId: "live", state: "exited", exit: { code: 1, signal: null } }));
    tabs.close("live");
    expect(tabs.store.getState().tabs).toHaveLength(0);
    // 다시 불러온 창(새 객체)의 목록에 남아 있어도 탭을 만들지 않는다.
    host.runs = [runInfo({ runId: "live", state: "exited" })];
    const again = new RunTabs();
    again.setDeps(deps());
    again.connect(host.client, "p1");
    await settle();
    expect(again.store.getState().tabs).toEqual([]);
    again.disposeAll();
  });
});
