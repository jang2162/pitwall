// @vitest-environment happy-dom
// Git 로그 툴 윈도우(13-m2-plan M2-14)를 프로젝트 창(App kind="project")에 가짜 Host 를 붙여 본다: ⌘9 로 열고 `git.log` 를 받아
// canvas 에 화면 범위만 그리고, 테마 전환에 다시 칠하고, 필터 바 · 브랜치 패널의 값이 `git.log` 의 `refs` · `filter` 로 그대로 가고,
// 고르기 · 상세 · 비교, 우클릭의 쓰기(브랜치 · cherry-pick · reset 확인 · 되돌리기), 라벨 끌어 놓기, ref 바뀜에 다시 받기.
// canvas 는 기록기(testing.ts)로 바꿔 그린 것을 본다. M2-15 의 액션(`git.showCommit` 등)은 레지스트리에 가짜로 둔다.

import { ActionRegistry } from "@pitwall/keymap";
import type {
  ConfigView,
  GitRefs,
  HostInfo,
  KeymapFile,
  LogChunk,
  LogRow,
  Project,
  ProjectTrust,
} from "@pitwall/rpc-client";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../../extensions/theme-default/themes/default.json";
import { App } from "../../App";
import { resetWindowFileTree } from "../../file-tree/project-view";
import type { KeymapNative } from "../../keymap/native";
import { activeWorktreeStore } from "../../open-with/active-worktree";
import { documentThemeApplier } from "../../theme/adapters";
import { createThemeStore, type ThemeStore } from "../../theme/theme-store";
import { ThemeStoreProvider } from "../../theme/theme-switcher";
import { parseTheme } from "../../theme/tokens";
import { resetWindowBranch } from "../branch-state";
import { resetWindowGitStatus } from "../git-status";
import { DEBOUNCE_MS } from "./filter-bar";
import { ROW_HEIGHT } from "./graph-render";
import { fixtureRows, hashOf, RecordingContext } from "./testing";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const host = vi.hoisted(() => ({
  calls: [] as { method: string; params: Record<string, unknown> }[],
  rows: [] as unknown[],
  /** 있으면 `git.log` 의 행을 이것으로 고른다(없거나 `undefined` 를 돌려주면 `rows`). 브랜치 거르기에서 빠지는 커밋을 흉내 낸다. */
  logRows: undefined as ((params: Record<string, unknown>) => unknown[] | undefined) | undefined,
  /** 메서드 → 답(없으면 기본). 함수면 params 로 부른다. */
  answers: new Map<string, unknown>(),
  emit: (_topic: string, _payload: unknown) => {},
}));

vi.mock("../../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../../keymap/default-keymap");
  const { i18nState } = await import("../../i18n/testing");
  return {
    connectHost: async () => {
      const [client, server] = pair();
      const subs = new Map<number, { projectId?: string; topics: string[] }>();
      let next = 1;
      const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
      const project: Project = {
        id: "p1",
        name: "demo",
        host: "local",
        roots: ["/work/demo"],
        favorite: false,
        tags: [],
        configMode: "repository",
        slug: "demo",
        createdAt: 1,
        openedAt: null,
      };
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const reply = (result: unknown) => send({ id: m.id, result });
        const params = (m.params ?? {}) as Record<string, unknown>;
        if (m.method.startsWith("git.")) host.calls.push({ method: m.method, params });
        const answer = host.answers.get(m.method);
        if (answer !== undefined) {
          const value =
            typeof answer === "function" ? (answer as (p: unknown) => unknown)(params) : answer;
          if (value && typeof value === "object" && "error" in value)
            return send({ id: m.id, error: (value as { error: unknown }).error });
          return reply(value);
        }
        switch (m.method) {
          case "i18n.get":
            return reply(i18nState("ko"));
          case "config.get":
            return reply({
              snapshot: { files: [], notices: [] },
              error: null,
            } satisfies ConfigView);
          case "keymap.get":
            return reply({
              id: "pitwall-default",
              extension: "dev.pitwall.keymap-default",
              kind: "base",
              content: DEFAULT_KEYMAP_TOML,
            } satisfies KeymapFile);
          case "host.info":
            return reply({
              hostId: "local",
              kind: "local",
              version: "0",
              os: "t",
              arch: "t",
            } satisfies HostInfo);
          case "task.list":
            return reply([]);
          case "project.get":
            return reply(project);
          case "project.trust":
            return reply({
              projectId: "p1",
              ownerId: "p1",
              state: "trusted",
            } satisfies ProjectTrust);
          case "project.storage":
            return reply({ kind: "ok", path: "/data/state.db" });
          case "project.claimTrustPrompt":
            return reply(false);
          case "fs.list":
            return reply({ path: params.path, entries: [] });
          case "git.watch": {
            const streamId = next++;
            send({ id: m.id, result: { streamId } });
            return send({
              method: "stream.chunk",
              params: { streamId, data: { workTree: "/work/demo" } },
            });
          }
          case "git.status":
            return reply({ workTree: "/work/demo", entries: [], unsupported: null });
          case "git.log": {
            const streamId = next++;
            send({ id: m.id, result: { streamId } });
            const rows = (host.logRows?.(params) ?? host.rows) as LogRow[];
            const filtered = Object.values((params.filter ?? {}) as Record<string, unknown>).some(
              (v) => v !== null && v !== false && !(Array.isArray(v) && v.length === 0),
            );
            const out = filtered ? rows.slice(0, 3).map((r) => ({ ...r, graph: null })) : rows;
            const chunks: LogChunk[] = [];
            for (let at = 0; at < Math.max(1, out.length); at += at === 0 ? 200 : 500) {
              const size = at === 0 ? 200 : 500;
              // 고정물은 pw-git 이 같은 크기(200 · 500)의 청크로 받은 것이라 청크의 첫 행에 `top` 이 있다.
              const slice = out.slice(at, at + size);
              chunks.push({ rows: slice, total: at === 0 && !filtered ? out.length : null });
              if (out.length === 0) break;
            }
            for (const data of chunks) send({ method: "stream.chunk", params: { streamId, data } });
            return send({ method: "stream.end", params: { streamId, status: "completed" } });
          }
          case "git.logPage": {
            // 해시 접두가 커밋으로 풀리는지만 묻는다(log-model.ts `resolvesAsHash`).
            const filter = (params.filter ?? {}) as { hash?: string | null };
            const rows = (host.rows as LogRow[]).filter(
              (r) => !filter.hash || r.hash.startsWith(filter.hash),
            );
            return reply({
              rows: rows.slice(0, params.limit as number),
              nextCursor: null,
              total: null,
            });
          }
          case "stream.cancel":
            return;
          case "events.subscribe": {
            const id = next++;
            subs.set(id, m.params as { projectId?: string; topics: string[] });
            return reply({ subscriptionId: id });
          }
          case "events.unsubscribe":
            subs.delete((m.params as { subscriptionId: number }).subscriptionId);
            return reply(null);
          default:
            return send({ id: m.id, error: { code: -32601, message: `no ${m.method}` } });
        }
      });
      host.emit = (topic, payload) => {
        for (const [subscriptionId, s] of subs) {
          if (s.projectId !== "p1" || !s.topics.includes(topic)) continue;
          send({
            method: "events.event",
            params: { subscriptionId, projectId: "p1", topic, payload },
          });
        }
      };
      return { transport: client, close: async () => {} };
    },
  };
});

const native: KeymapNative = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
  closeWindow: async () => {},
};

const REFS: GitRefs = {
  workTree: "/work/demo",
  head: { kind: "branch", name: "main", commit: hashOf(0) },
  branches: [
    {
      name: "feat/x",
      refName: "refs/heads/feat/x",
      commit: hashOf(2),
      head: false,
      upstream: null,
    },
    { name: "main", refName: "refs/heads/main", commit: hashOf(0), head: true, upstream: null },
  ],
  remoteBranches: [
    { remote: "origin", name: "origin/old", refName: "refs/remotes/origin/old", commit: hashOf(4) },
  ],
  tags: [{ name: "v1", refName: "refs/tags/v1", target: hashOf(4), peeled: null }],
  unsupported: null,
  broken: [],
};

const VIEW = { width: 900, height: 10 * ROW_HEIGHT };
let root: Root | undefined;
let theme: ThemeStore | undefined;
let ctx: RecordingContext;
/** M2-15 의 문서 액션을 부른 것(id · 인자). */
let ran: { id: string; args: unknown }[];
const GIT_DOCUMENT_ACTIONS = ["git.showCommit", "git.compareCommits", "git.openRevisionFile"];

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  const isList = (el: HTMLElement) => el.dataset.testid === "git-log-list";
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockImplementation(function (
    this: HTMLElement,
  ) {
    return isList(this) ? VIEW.height : this.getAttribute("role") === "tree" ? 400 : 0;
  });
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockImplementation(function (
    this: HTMLElement,
  ) {
    return isList(this) ? VIEW.width : 0;
  });
  vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockImplementation(function (
    this: HTMLElement,
  ) {
    return this.getAttribute("role") === "tree" ? 400 : 0;
  });
  ctx = new RecordingContext();
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(
    () => ctx as unknown as CanvasRenderingContext2D,
  );
  // M2-15 의 문서 액션은 프로젝트 창이 실제로 등록한다(인자가 있어야 도는 액션 — `isEnabled` 는 거짓). 탭을 여는 실행만 막고 부른
  // 인자를 남긴다.
  ran = [];
  const realRun = ActionRegistry.prototype.run;
  vi.spyOn(ActionRegistry.prototype, "run").mockImplementation(function (
    this: ActionRegistry,
    id,
    args,
  ) {
    if (!GIT_DOCUMENT_ACTIONS.includes(id)) return realRun.call(this, id, args);
    if (!this.isAvailable(id)) return false;
    ran.push({ id, args });
    return true;
  });
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.calls = [];
  host.rows = fixtureRows("synth");
  host.logRows = undefined;
  host.answers = new Map<string, unknown>([
    ["git.refs", REFS],
    // 창의 브랜치 상태(branch-state.ts)가 ref 와 함께 읽는다.
    ["git.operation", { operation: null }],
    [
      "git.show",
      (p: { rev: string }) => ({
        commit: p.rev,
        tree: "t",
        parents: [hashOf(1)],
        shallow: false,
        author: { name: "Kim", email: "kim@example.com", time: 1_700_000_000, offset: 0 },
        committer: { name: "Kim", email: "kim@example.com", time: 1_700_000_000, offset: 0 },
        message: "feat: add invoice\n\nbody\n",
        head: false,
        refs: [],
      }),
    ],
    [
      "git.diff",
      () => ({
        workTree: "/work/demo",
        oldCommit: null,
        newCommit: null,
        files: [
          diffFile("src/main/App.kt", "modified"),
          diffFile("src/main/Repo.kt", "added"),
          diffFile("Cargo.lock", "modified", true),
        ],
      }),
    ],
    [
      "git.containedIn",
      { refs: [{ name: "main", fullName: "refs/heads/main", kind: "localBranch" }] },
    ],
  ]);
  activeWorktreeStore.setState({ top: undefined });
  resetWindowFileTree();
  resetWindowGitStatus();
  resetWindowBranch();
  localStorage.clear();
});

afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
  theme?.dispose();
  theme = undefined;
  document.body.innerHTML = "";
  document.documentElement.removeAttribute("style");
  vi.restoreAllMocks();
});

function diffFile(path: string, status: "added" | "modified", generated = false) {
  return {
    status,
    path,
    oldPath: null,
    oldMode: null,
    newMode: null,
    oldId: null,
    newId: null,
    binary: false,
    tooLarge: false,
    hunksOmitted: false,
    generated,
    submodule: null,
    added: 1,
    removed: 0,
    hunks: [],
    error: null,
  };
}

const settle = (ms = 30) => act(async () => new Promise((r) => setTimeout(r, ms)));

async function mount() {
  const element = document.createElement("div");
  document.body.append(element);
  theme = createThemeStore({
    system: { current: () => "light", subscribe: () => () => {} },
    storage: { read: () => undefined, write: () => {} },
    theme: parseTheme(defaultTheme),
    apply: documentThemeApplier(document.documentElement),
  });
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <ThemeStoreProvider store={theme!}>
        <App kind="project" native={native} />
      </ThemeStoreProvider>,
    ),
  );
  await settle();
}

async function press(init: KeyboardEventInit, target?: Element | null) {
  await act(async () => {
    (target ?? document.activeElement ?? document.body).dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
    );
    document.body.dispatchEvent(new KeyboardEvent("keyup", { bubbles: true, ...init }));
  });
  await settle();
}

const list = () => document.querySelector<HTMLElement>("[data-testid=git-log-list]");
const logCalls = () => host.calls.filter((c) => c.method === "git.log");
const subjects = () => ctx.texts.map((t) => t.text).filter((t) => t.startsWith("subject "));
/** 간선(노드의 테두리 원은 점이 없다). */
const edges = () => ctx.strokes.filter((s) => s.points.length > 0);

async function openLog() {
  await mount();
  await press({ code: "Digit9", key: "9", metaKey: true });
  await vi.waitFor(() => expect(list()).not.toBeNull());
  await vi.waitFor(() => expect(subjects().length).toBeGreaterThan(0));
}

/** 목록의 행 `index` 의 가운데(행 높이 기준 — happy-dom 은 배치가 없어 목록의 왼쪽 위가 0, 0 이다). */
const rowY = (index: number) => index * ROW_HEIGHT + ROW_HEIGHT / 2 - (list()?.scrollTop ?? 0);

async function pointer(type: string, x: number, y: number, init: PointerEventInit = {}) {
  await act(async () => {
    list()!.dispatchEvent(
      new PointerEvent(type, {
        bubbles: true,
        cancelable: true,
        pointerId: 1,
        button: 0,
        clientX: x,
        clientY: y,
        ...init,
      }),
    );
  });
  await settle();
}

async function clickRow(index: number, init: PointerEventInit = {}) {
  await pointer("pointerdown", 400, rowY(index), init);
  await pointer("pointerup", 400, rowY(index), init);
}

async function mouse(el: Element, type: "click" | "dblclick" | "contextmenu", x = 400, y = 0) {
  await act(async () => {
    el.dispatchEvent(
      new MouseEvent(type, {
        bubbles: true,
        cancelable: true,
        button: type === "contextmenu" ? 2 : 0,
        clientX: x,
        clientY: y,
      }),
    );
  });
  await settle();
}

const buttonText = (text: string) =>
  [...document.querySelectorAll<HTMLElement>("button")].find((b) => b.textContent === text);

const menuItem = (text: string) =>
  [...document.querySelectorAll<HTMLElement>("[role=menuitem]")].find((e) =>
    e.textContent?.startsWith(text),
  );

async function answerName(name: string) {
  const input = await vi.waitFor(() => {
    const el = document.querySelector<HTMLInputElement>("[data-testid=git-branch-dialog] input");
    if (!el) throw new Error("no name dialog");
    return el;
  });
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, name);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await press({ key: "Enter", code: "Enter" }, input);
}

async function answerConfirm(label: string): Promise<string> {
  const dialog = await vi.waitFor(() => {
    const el = document.querySelector<HTMLElement>("[data-testid=git-confirm-dialog]");
    if (!el) throw new Error("no confirm dialog");
    return el;
  });
  const text = dialog.textContent ?? "";
  const button = [...dialog.querySelectorAll("button")].find((b) => b.textContent === label)!;
  await mouse(button, "click");
  return text;
}

it("opens with ⌘9, streams git.log for this project and draws only the rows on screen", async () => {
  await openLog();
  expect(logCalls()[0]!.params).toEqual({
    projectId: "p1",
    worktree: null,
    refs: null,
    filter: {
      author: null,
      message: null,
      regex: false,
      ignoreCase: false,
      since: null,
      until: null,
      paths: [],
      hash: null,
    },
    order: "topo",
  });
  // 300 행 중 화면(10 행)에 든 것만 글자를 그린다.
  expect(subjects()).toEqual(Array.from({ length: 10 }, (_, i) => `subject ${i}`));
  expect(document.querySelector("[data-testid=git-log-status]")!.textContent).toContain(
    "커밋 300개",
  );
  // 스크롤 범위는 행 수 × 행 높이다(canvas 높이 + 빈 칸).
  const spacer = list()!.lastElementChild as HTMLElement;
  expect(Number.parseFloat(spacer.style.height) + VIEW.height).toBe(300 * ROW_HEIGHT);

  // 굴리면 그 범위만 다시 그린다.
  ctx.clear();
  await act(async () => {
    list()!.scrollTop = 100 * ROW_HEIGHT;
    list()!.dispatchEvent(new Event("scroll"));
  });
  await settle();
  expect(subjects()).toEqual(Array.from({ length: 10 }, (_, i) => `subject ${100 + i}`));
});

it("colors lanes from the graph.lane tokens and repaints on a theme switch", async () => {
  await openLog();
  const light = defaultTheme.variants.light as Record<string, string>;
  const dark = defaultTheme.variants.dark as Record<string, string>;
  const laneColors = (values: Record<string, string>) =>
    Array.from({ length: 8 }, (_, i) => values[`graph.lane.${i + 1}`]!);
  expect(edges().length).toBeGreaterThan(0);
  expect(edges().every((s) => laneColors(light).includes(s.style))).toBe(true);
  expect(ctx.fills[0]!.style).toBe(light["ui.panelBackground"]);

  ctx.clear();
  await act(async () => theme!.getState().setPreference("dark"));
  await settle();
  expect(ctx.fills[0]!.style).toBe(dark["ui.panelBackground"]);
  expect(edges().length).toBeGreaterThan(0);
  expect(edges().every((s) => laneColors(dark).includes(s.style))).toBe(true);
});

it("paints the folded lane with graph.lane.folded, not the palette's remainder", async () => {
  host.rows = fixtureRows("fanout");
  await openLog();
  const light = defaultTheme.variants.light as Record<string, string>;
  // 갈래가 레인 상한을 넘는 고정물: 64 번째 갈래부터 접힌 레인이다.
  ctx.clear();
  await act(async () => {
    list()!.scrollTop = 70 * ROW_HEIGHT;
    list()!.dispatchEvent(new Event("scroll"));
  });
  await settle();
  const folded = edges().filter((s) => s.style === light["graph.lane.folded"]);
  expect(folded.length).toBeGreaterThan(0);
  // 접힌 레인(64)의 간선만 그 색이다 — 팔레트 나머지(4294967295 % 8 = 7 → graph.lane.8)로 칠하지 않는다.
  const foldedX = 6 + 64 * 12 + 6;
  expect(folded.every((s) => s.points.some(([x]) => x === foldedX))).toBe(true);
  expect(
    edges().some(
      (s) => s.style === light["graph.lane.8"] && s.points.every(([x]) => x === foldedX),
    ),
  ).toBe(false);
});

it("emphasises the hovered line and dims the rest", async () => {
  await openLog();
  ctx.clear();
  // 첫 행의 노드(레인 1 — 레인 0 은 아래의 HEAD 줄 몫이다)를 가리킨다.
  await pointer("pointermove", 6 + 12 + 6, rowY(0));
  await vi.waitFor(() => expect(edges().some((s) => s.width === 3)).toBe(true));
  const strong = edges().filter((s) => s.width === 3);
  const dim = edges().filter((s) => s.width !== 3);
  expect(new Set(strong.map((s) => s.style)).size).toBe(1);
  expect(dim.every((s) => s.alpha < 1)).toBe(true);
});

it("sends the filter bar and the branch panel as git.log refs and filter", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  try {
    await openLog();
    const text = document.querySelector<HTMLInputElement>('input[aria-label="텍스트/해시"]')!;
    const type = async (input: HTMLInputElement, value: string) => {
      await act(async () => {
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(
          input,
          value,
        );
        input.dispatchEvent(new Event("input", { bubbles: true }));
      });
      await act(async () => vi.advanceTimersByTime(DEBOUNCE_MS + 10));
      await settle();
    };
    await type(text, "fix: (null|nil)");
    await mouse(document.querySelector('button[aria-label="정규식"]')!, "click");
    await mouse(document.querySelector('button[aria-label="대소문자 구분"]')!, "click");
    expect(logCalls().at(-1)!.params.filter).toEqual({
      author: null,
      message: "fix: (null|nil)",
      regex: true,
      ignoreCase: true,
      since: null,
      until: null,
      paths: [],
      hash: null,
    });
    // 걸러진 목록은 그래프가 없다(결정 M2-D2) — 그렇다고 알린다.
    expect(document.querySelector("[data-testid=git-log-status]")!.textContent).toContain(
      "그래프 없이",
    );

    // 16진 낱말 · 이슈 번호는 메시지다(해시로 묻지도 않는다 — 7자 아래).
    for (const word of ["added", "2024"]) {
      await type(text, word);
      expect(logCalls().at(-1)!.params.filter).toMatchObject({ message: word, hash: null });
    }
    expect(host.calls.filter((c) => c.method === "git.logPage")).toEqual([]);
    // 7자 이상 16진이어도 그런 커밋이 없으면 메시지다.
    await type(text, "deadbeef");
    expect(host.calls.filter((c) => c.method === "git.logPage").at(-1)!.params).toMatchObject({
      refs: null,
      filter: { hash: "deadbeef", message: null },
      limit: 1,
    });
    expect(logCalls().at(-1)!.params.filter).toMatchObject({ message: "deadbeef", hash: null });
    expect(document.querySelector("[data-testid=git-log-status]")!.textContent).not.toContain(
      "해시",
    );
    // 커밋으로 풀리면 해시 접두이고, 상태 줄이 그렇게 알린다.
    const prefix = hashOf(3).toUpperCase();
    await type(text, prefix);
    expect(logCalls().at(-1)!.params.filter).toMatchObject({
      message: null,
      hash: hashOf(3),
    });
    await vi.waitFor(() =>
      expect(document.querySelector("[data-testid=git-log-status]")!.textContent).toContain(
        "해시 접두로 찾은 커밋",
      ),
    );

    // 작성자 · 날짜 · 경로.
    await mouse(
      [...document.querySelectorAll("button")].find((b) => b.textContent === "작성자")!,
      "click",
    );
    await type(document.querySelector<HTMLInputElement>('input[aria-label="작성자"]')!, "kim");
    await mouse(
      [...document.querySelectorAll("button")].find((b) => b.textContent === "날짜")!,
      "click",
    );
    const since = document.querySelector<HTMLInputElement>('input[aria-label="부터"]')!;
    await type(since, "2026-10-01");
    await mouse(
      [...document.querySelectorAll("button")].find((b) => b.textContent === "경로")!,
      "click",
    );
    await type(
      document.querySelector<HTMLInputElement>('input[aria-label="경로"]')!,
      "src/main, docs",
    );
    expect(logCalls().at(-1)!.params.filter).toEqual({
      author: "kim",
      message: null,
      regex: true,
      ignoreCase: true,
      since: Math.floor(new Date(2026, 9, 1).getTime() / 1000),
      until: null,
      paths: ["src/main", "docs"],
      hash: hashOf(3),
    });

    // 필터 지우기 → 처음 요청과 같다.
    await mouse(document.querySelector('button[aria-label="필터 지우기"]')!, "click");
    expect(logCalls().at(-1)!.params.filter).toEqual(logCalls()[0]!.params.filter);

    // 브랜치 패널: 클릭 = 그것 하나, ⌘+클릭 = 더하기, 다시 클릭 = 풀기. 넘기는 것은 ref 전체 이름.
    const branch = (name: string) => document.querySelector<HTMLElement>(`[data-ref="${name}"]`)!;
    await vi.waitFor(() => expect(branch("refs/heads/main")).not.toBeNull());
    await mouse(branch("refs/heads/main"), "click");
    expect(logCalls().at(-1)!.params.refs).toEqual(["refs/heads/main"]);
    await act(async () => {
      branch("refs/heads/feat/x").dispatchEvent(
        new MouseEvent("click", { bubbles: true, metaKey: true }),
      );
    });
    await settle();
    expect(logCalls().at(-1)!.params.refs).toEqual(["refs/heads/main", "refs/heads/feat/x"]);
    await mouse(branch("refs/heads/feat/x"), "click");
    expect(logCalls().at(-1)!.params.refs).toEqual(["refs/heads/feat/x"]);
    await mouse(branch("refs/heads/feat/x"), "click");
    expect(logCalls().at(-1)!.params.refs).toBeNull();
  } finally {
    vi.useRealTimers();
  }
});

it("shows a commit's files, message, parents and branches, and compares two commits", async () => {
  await openLog();
  await clickRow(2);
  await vi.waitFor(() =>
    expect(document.querySelector("[data-testid=git-log-contained]")?.textContent).toBe("main"),
  );
  const detail = document.querySelector<HTMLElement>("[data-testid=git-log-detail]")!;
  expect(detail.textContent).toContain("변경 파일 (3)");
  expect(detail.textContent).toContain("src/main");
  expect(detail.textContent).toContain("App.kt");
  expect(detail.textContent).toContain("feat: add invoice");
  expect(detail.textContent).toContain("생성");
  for (const method of ["git.show", "git.containedIn"])
    expect(host.calls.find((c) => c.method === method)!.params).toMatchObject({
      projectId: "p1",
      rev: hashOf(2),
    });
  expect(host.calls.find((c) => c.method === "git.diff")!.params).toMatchObject({
    to: { kind: "commit", rev: hashOf(2) },
  });
  // 선택한 행은 칠한다.
  expect(ctx.fills.some((f) => f.y === 2 * ROW_HEIGHT && f.h === ROW_HEIGHT)).toBe(true);

  // 부모를 누르면 그 커밋을 고른다.
  const parent = [...detail.querySelectorAll("button")].find(
    (b) => b.textContent === hashOf(1).slice(0, 8),
  )!;
  await mouse(parent, "click");
  await vi.waitFor(() =>
    expect(host.calls.filter((c) => c.method === "git.show").at(-1)!.params.rev).toBe(hashOf(1)),
  );

  // ⌘+클릭으로 둘 → 비교(옛 쪽 → 새 쪽).
  await clickRow(5, { metaKey: true });
  await vi.waitFor(() =>
    expect(document.querySelector("[data-testid=git-log-compare]")).not.toBeNull(),
  );
  expect(host.calls.filter((c) => c.method === "git.diff").at(-1)!.params).toMatchObject({
    from: { kind: "commit", rev: hashOf(5) },
    to: { kind: "commit", rev: hashOf(1) },
  });
  const open = [...document.querySelectorAll<HTMLElement>("button")].find(
    (b) => b.textContent === "비교 탭으로 열기",
  )!;
  await mouse(open, "click");
  expect(ran.at(-1)).toEqual({ id: "git.compareCommits", args: { a: hashOf(5), b: hashOf(1) } });

  // ⇧+클릭은 범위.
  await clickRow(3);
  await clickRow(6, { shiftKey: true });
  await vi.waitFor(() => expect(document.querySelector("[data-testid=git-log-detail]")).toBeNull());
  expect(document.body.textContent).toContain("커밋 4개를 골랐다");
});

it("opens the commit details tab on double click and Enter, and copies the hash with ⌘C", async () => {
  const writeText = vi.fn(async (_text: string) => {});
  Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
  try {
    await openLog();
    await mouse(list()!, "dblclick", 400, rowY(3));
    expect(ran.at(-1)).toEqual({ id: "git.showCommit", args: { rev: hashOf(3) } });
    await clickRow(7);
    list()!.focus();
    await press({ key: "ArrowDown", code: "ArrowDown" }, list());
    await press({ key: "Enter", code: "Enter" }, list());
    expect(ran.at(-1)).toEqual({ id: "git.showCommit", args: { rev: hashOf(8) } });
    await press({ key: "c", code: "KeyC", metaKey: true }, list());
    expect(writeText).toHaveBeenCalledWith(hashOf(8));
  } finally {
    Reflect.deleteProperty(navigator, "clipboard");
  }
});

it("creates a branch and cherry-picks the selected commits oldest first from the context menu", async () => {
  host.answers.set("git.branch.create", {
    workTree: "/work/demo",
    refName: "refs/heads/topic",
    commit: hashOf(3),
    head: null,
  });
  host.answers.set("git.cherryPick", {
    workTree: "/work/demo",
    outcome: "done",
    head: "f".repeat(40),
    moved: { refName: "refs/heads/main", previous: hashOf(0), current: "f".repeat(40) },
    operation: null,
  });
  await openLog();
  await mouse(list()!, "contextmenu", 400, rowY(3));
  await mouse(menuItem("새 브랜치…")!, "click");
  await answerName("topic");
  expect(host.calls.find((c) => c.method === "git.branch.create")!.params).toEqual({
    projectId: "p1",
    worktree: null,
    name: "topic",
    start: hashOf(3),
  });
  // 만든 브랜치에도 [되돌리기] 띠가 선다(지운다 — 그사이 움직였으면 거절).
  await vi.waitFor(() => {
    const row = [...document.querySelectorAll<HTMLElement>("[role=status]")].find((e) =>
      e.textContent?.startsWith("topic: ∅ →"),
    );
    expect(row?.textContent).toContain("되돌리기");
  });

  await clickRow(3);
  await clickRow(5, { metaKey: true });
  await mouse(list()!, "contextmenu", 400, rowY(5));
  await mouse(menuItem("커밋 2개 Cherry-pick")!, "click");
  await vi.waitFor(() =>
    expect(host.calls.find((c) => c.method === "git.cherryPick")!.params.commits).toEqual([
      hashOf(5),
      hashOf(3),
    ]),
  );
  // ref 가 움직였으니 [되돌리기] 줄이 선다.
  await vi.waitFor(() => expect(document.body.textContent).toContain("main: "));
});

it("turns off cherry-pick and revert when a merge commit is among the chosen", async () => {
  // 넷째 행을 머지 커밋으로(부모 둘). 그래프는 그대로 — 메뉴는 부모 수만 본다.
  host.rows = fixtureRows("synth").map((r, i) =>
    i === 3 ? { ...r, parents: [hashOf(4), hashOf(9)] } : r,
  );
  await openLog();
  await mouse(list()!, "contextmenu", 400, rowY(3));
  expect(menuItem("Cherry-pick")!.getAttribute("data-disabled")).not.toBeNull();
  expect(menuItem("커밋 되돌리기(Revert)")!.getAttribute("data-disabled")).not.toBeNull();
  await press({ key: "Escape", code: "Escape" });
  await mouse(list()!, "contextmenu", 400, rowY(2));
  expect(menuItem("Cherry-pick")!.getAttribute("data-disabled")).toBeNull();
  expect(menuItem("커밋 되돌리기(Revert)")!.getAttribute("data-disabled")).toBeNull();
});

it("shows the details · compare entries only while M2-15's actions are available, and calls them with registry.run", async () => {
  await openLog();
  // 프로젝트 창이 등록한 액션(인자가 있어야 도는 액션 — isEnabled 는 늘 거짓)이면 메뉴 · 버튼이 있다.
  await clickRow(2);
  await vi.waitFor(() => expect(buttonText("커밋 상세 탭으로 열기")).toBeDefined());
  await mouse(list()!, "contextmenu", 400, rowY(2));
  expect(menuItem("커밋 상세 탭으로 열기")).toBeDefined();
  expect(menuItem("두 커밋 비교 탭으로 열기")).toBeDefined();
  await press({ key: "Escape", code: "Escape" });

  // 그 창에 액션이 없으면(꺼졌으면) 감추고, 더블클릭도 아무것도 부르지 않는다.
  const realAvailable = ActionRegistry.prototype.isAvailable;
  vi.spyOn(ActionRegistry.prototype, "isAvailable").mockImplementation(function (
    this: ActionRegistry,
    id,
  ) {
    return GIT_DOCUMENT_ACTIONS.includes(id) ? false : realAvailable.call(this, id);
  });
  await clickRow(4);
  await vi.waitFor(() =>
    expect(document.querySelector("[data-testid=git-log-detail]")).not.toBeNull(),
  );
  expect(buttonText("커밋 상세 탭으로 열기")).toBeUndefined();
  await mouse(list()!, "contextmenu", 400, rowY(4));
  expect(menuItem("새 브랜치…")).toBeDefined();
  expect(menuItem("커밋 상세 탭으로 열기")).toBeUndefined();
  expect(menuItem("두 커밋 비교 탭으로 열기")).toBeUndefined();
  await press({ key: "Escape", code: "Escape" });
  const before = ran.length;
  await mouse(list()!, "dblclick", 400, rowY(4));
  expect(ran.length).toBe(before);
});

it("resets hard only after confirming the lost changes, then undoes back with reset --keep", async () => {
  const head = hashOf(0);
  // 되돌리기(keep)의 미리보기는 잃을 것이 없다 — 묻지 않고 그 지문으로 부른다.
  host.answers.set("git.resetPreview", (p: { mode: string }) =>
    p.mode === "keep"
      ? {
          workTree: "/work/demo",
          head: hashOf(4),
          target: head,
          lost: { changes: [], overwritten: [] },
          fingerprint: "fpk",
        }
      : {
          workTree: "/work/demo",
          head,
          target: hashOf(4),
          lost: {
            changes: [
              {
                path: "src/a.rs",
                origPath: null,
                staged: null,
                unstaged: "modified",
                conflict: null,
              },
            ],
            overwritten: ["build/out.txt"],
          },
          fingerprint: "fp1",
        },
  );
  host.answers.set("git.reset", (p: { mode: string }) => ({
    workTree: "/work/demo",
    head: p.mode === "keep" ? head : hashOf(4),
    moved:
      p.mode === "keep"
        ? { refName: "refs/heads/main", previous: hashOf(4), current: head }
        : { refName: "refs/heads/main", previous: head, current: hashOf(4) },
    lost: null,
  }));
  await openLog();
  await mouse(list()!, "contextmenu", 400, rowY(4));
  await act(async () => {
    menuItem("이 커밋으로 Reset")!.dispatchEvent(
      new PointerEvent("pointermove", { bubbles: true, pointerType: "mouse" }),
    );
    menuItem("이 커밋으로 Reset")!.dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, key: "ArrowRight" }),
    );
  });
  await settle();
  await mouse(menuItem("이 커밋으로 Reset --hard")!, "click");
  expect(host.calls.find((c) => c.method === "git.resetPreview")!.params).toMatchObject({
    commit: hashOf(4),
    mode: "hard",
  });
  // 확인 전에는 reset 하지 않는다.
  expect(host.calls.some((c) => c.method === "git.reset")).toBe(false);
  const text = await answerConfirm("Reset");
  expect(text).toContain("이 변경 2개를 잃는다");
  expect(text).toContain("src/a.rs");
  expect(text).toContain("build/out.txt");
  await vi.waitFor(() =>
    expect(host.calls.find((c) => c.method === "git.reset")!.params).toEqual({
      projectId: "p1",
      worktree: null,
      commit: hashOf(4),
      mode: "hard",
      expected: head,
      fingerprint: "fp1",
    }),
  );

  const undo = await vi.waitFor(() => {
    const b = [...document.querySelectorAll<HTMLElement>("button")].find(
      (e) => e.textContent === "되돌리기",
    );
    if (!b) throw new Error("no undo");
    return b;
  });
  await mouse(undo, "click");
  await vi.waitFor(() =>
    expect(host.calls.filter((c) => c.method === "git.reset").at(-1)!.params).toEqual({
      projectId: "p1",
      worktree: null,
      commit: head,
      mode: "keep",
      expected: hashOf(4),
      expectedRef: "refs/heads/main",
      fingerprint: "fpk",
    }),
  );
  expect(host.calls.filter((c) => c.method === "git.resetPreview").at(-1)!.params).toMatchObject({
    commit: head,
    mode: "keep",
  });
});

it("cancels a hard reset when the confirmation is refused", async () => {
  host.answers.set("git.resetPreview", {
    workTree: "/work/demo",
    head: hashOf(0),
    target: hashOf(4),
    lost: { changes: [], overwritten: [] },
    fingerprint: "fp",
  });
  await openLog();
  await mouse(list()!, "contextmenu", 400, rowY(4));
  await act(async () => {
    menuItem("이 커밋으로 Reset")!.dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, key: "ArrowRight" }),
    );
  });
  await settle();
  await mouse(menuItem("이 커밋으로 Reset --hard")!, "click");
  const text = await answerConfirm("취소");
  expect(text).toContain("잃는 것이 없다");
  await settle();
  expect(host.calls.some((c) => c.method === "git.reset")).toBe(false);
});

it("rebases a dragged branch label onto the label it is dropped on", async () => {
  host.answers.set("git.rebase", {
    workTree: "/work/demo",
    outcome: "done",
    head: hashOf(2),
    moved: null,
    operation: null,
  });
  await openLog();
  // 셋째 행의 라벨 `feat/x` 는 제목 칸 맨 앞이다.
  const labelX = (y: number) => {
    const label = ctx.texts.find((t) => t.y === y && t.text === "feat/x");
    if (!label) throw new Error("no label drawn");
    return label.x + 2;
  };
  const from = labelX(rowY(2));
  const to = ctx.texts.find((t) => t.y === rowY(0) && t.text === "● main")!.x + 2;
  await pointer("pointerdown", from, rowY(2));
  await pointer("pointermove", from + 10, rowY(1));
  await pointer("pointermove", to, rowY(0));
  await pointer("pointerup", to, rowY(0));
  await vi.waitFor(() => expect(menuItem("feat/x 를 main 위로 rebase")).toBeDefined());
  // 놓은 자리가 지금 브랜치라 merge 도 된다.
  expect(menuItem("feat/x 를 main 에 merge")!.getAttribute("data-disabled")).toBeNull();
  await mouse(menuItem("feat/x 를 main 위로 rebase")!, "click");
  await vi.waitFor(() =>
    expect(host.calls.find((c) => c.method === "git.rebase")!.params).toEqual({
      projectId: "p1",
      worktree: null,
      upstream: "refs/heads/main",
      branch: "feat/x",
    }),
  );
});

it("passes the full ref name when a branch and a tag share a short name", async () => {
  // 브랜치 `v1`(셋째 행)과 태그 `v1`(다섯째 행). 짧은 이름이면 git 은 태그를 먼저 푼다.
  host.rows = fixtureRows("synth").map((r, i) =>
    i === 2 ? { ...r, refs: [{ name: "v1", kind: "branch", current: false }] } : r,
  );
  host.answers.set("git.refs", {
    ...REFS,
    branches: [
      ...REFS.branches,
      { name: "v1", refName: "refs/heads/v1", commit: hashOf(2), head: false, upstream: null },
    ],
  });
  host.answers.set("git.merge", {
    workTree: "/work/demo",
    outcome: "done",
    head: hashOf(0),
    moved: null,
    operation: null,
  });
  await openLog();
  // 라벨 끌기: 브랜치 `v1` 을 지금 브랜치 `main` 에 놓고 merge · 비교.
  const dragV1ToMain = async () => {
    const label = ctx.texts.find((t) => t.y === rowY(2) && t.text === "v1")!;
    const to = ctx.texts.find((t) => t.y === rowY(0) && t.text === "● main")!.x + 2;
    await pointer("pointerdown", label.x + 2, rowY(2));
    await pointer("pointermove", label.x + 12, rowY(1));
    await pointer("pointermove", to, rowY(0));
    await pointer("pointerup", to, rowY(0));
    await vi.waitFor(() => expect(menuItem("v1 를 main 에 merge")).toBeDefined());
  };
  await dragV1ToMain();
  await mouse(menuItem("v1 를 main 에 merge")!, "click");
  await vi.waitFor(() =>
    expect(host.calls.find((c) => c.method === "git.merge")!.params).toEqual({
      projectId: "p1",
      worktree: null,
      rev: "refs/heads/v1",
    }),
  );
  await dragV1ToMain();
  await mouse(
    [...document.querySelectorAll<HTMLElement>("[role=menuitem]")].find((e) =>
      e.textContent?.includes("비교"),
    )!,
    "click",
  );
  expect(ran.at(-1)).toEqual({
    id: "git.compareCommits",
    args: { a: "refs/heads/main", b: "refs/heads/v1" },
  });
  // 브랜치 패널: 로컬 `v1` 과 태그 `v1` 은 다른 값으로 거른다.
  const panelRef = (value: string) => document.querySelector<HTMLElement>(`[data-ref="${value}"]`);
  await mouse(panelRef("refs/heads/v1")!, "click");
  expect(logCalls().at(-1)!.params.refs).toEqual(["refs/heads/v1"]);
  await mouse(
    [...document.querySelectorAll<HTMLElement>("[role=treeitem]")].find((e) =>
      e.textContent?.startsWith("태그"),
    )!,
    "click",
  );
  await vi.waitFor(() => expect(panelRef("refs/tags/v1")).not.toBeNull());
  await mouse(panelRef("refs/tags/v1")!, "click");
  expect(logCalls().at(-1)!.params.refs).toEqual(["refs/tags/v1"]);
  // 필터 바의 브랜치 ▾ 도 같은 값(전체 이름)을 더한다.
  await mouse(
    [...document.querySelectorAll("button")].find((b) => b.textContent?.startsWith("브랜치"))!,
    "click",
  );
  const v1Box = [...document.querySelectorAll("label")]
    .find((l) => l.textContent === "v1")!
    .querySelector("input")!;
  await mouse(v1Box, "click");
  expect(logCalls().at(-1)!.params.refs).toEqual(["refs/tags/v1", "refs/heads/v1"]);
});

it("repaints at the new pixel ratio when the window moves to another display", async () => {
  const changes: (() => void)[] = [];
  const realMatchMedia = window.matchMedia.bind(window);
  vi.spyOn(window, "matchMedia").mockImplementation((query: string) => {
    if (!query.includes("dppx")) return realMatchMedia(query);
    return {
      matches: true,
      media: query,
      addEventListener: (_: string, fn: () => void) => changes.push(fn),
      removeEventListener: () => {},
    } as unknown as MediaQueryList;
  });
  const ratio = vi.spyOn(window, "devicePixelRatio", "get").mockReturnValue(1);
  await openLog();
  const canvas = list()!.querySelector("canvas")!;
  expect(canvas.width).toBe(VIEW.width);
  ratio.mockReturnValue(2);
  await act(async () => {
    for (const fn of changes.splice(0)) fn();
  });
  await vi.waitFor(() => expect(canvas.width).toBe(VIEW.width * 2));
  expect(canvas.height).toBe(VIEW.height * 2);
});

it("reloads the log with the same filter when refs change", async () => {
  await openLog();
  const before = logCalls().length;
  await act(async () => host.emit("git.refsChanged", { worktree: null }));
  await settle();
  expect(logCalls().length).toBe(before + 1);
  // 다른 워크트리의 바뀜은 무시한다.
  await act(async () => host.emit("git.refsChanged", { worktree: "/work/other" }));
  await settle();
  expect(logCalls().length).toBe(before + 1);
});

// 「Git 로그에서 선택」(M2-18 — log-reveal.ts): 다른 화면(파일 히스토리 · blame 주석)이 `registry.run("git.log.reveal", { rev })` 로
// 부른다.

/** 창의 액션 레지스트리를 잡는다(마운트 전에 부른다). */
function captureRegistry(): () => ActionRegistry | undefined {
  const seen = new Set<ActionRegistry>();
  const register = ActionRegistry.prototype.register;
  vi.spyOn(ActionRegistry.prototype, "register").mockImplementation(function (
    this: ActionRegistry,
    action,
  ) {
    seen.add(this);
    return register.call(this, action);
  });
  return () => [...seen].at(-1);
}

/** 상세가 그 커밋의 바뀐 파일을 물었는가(고른 커밋이 하나이고 그것이다). */
const detailAsked = (hash: string) =>
  host.calls.some(
    (c) =>
      c.method === "git.diff" &&
      (c.params.to as { rev?: string } | undefined)?.rev === hash &&
      c.params.from === undefined,
  );

it("git.log.reveal opens the log, selects the commit, scrolls it into view and focuses the list", async () => {
  const registry = captureRegistry();
  await mount();
  await vi.waitFor(() => expect(registry()?.get("git.log.reveal")).toBeDefined());
  // 인자가 있어야만 돈다 — 키 · 액션 찾기에서는 부를 수 없다.
  expect(registry()!.isEnabled("git.log.reveal")).toBe(false);
  expect(list()).toBeNull();
  const hash = hashOf(40);
  await act(async () => void registry()!.run("git.log.reveal", { rev: hash }));
  await vi.waitFor(() => expect(list()).not.toBeNull());
  // 리비전은 커밋 해시로 푼다.
  expect(host.calls.find((c) => c.method === "git.show")!.params.rev).toBe(hash);
  await vi.waitFor(() => expect(detailAsked(hash)).toBe(true));
  await vi.waitFor(() => expect(document.activeElement).toBe(list()));
  // 행 40 이 화면(10행) 안으로 굴렀다.
  const top = list()!.scrollTop;
  expect(40 * ROW_HEIGHT).toBeGreaterThanOrEqual(top);
  expect(41 * ROW_HEIGHT).toBeLessThanOrEqual(top + VIEW.height);
  // 필터를 건드리지 않았다 — 한 번만 받았다.
  expect(logCalls()).toHaveLength(1);
});

it("git.log.reveal clears a branch filter that leaves the commit out, reloads and selects it", async () => {
  const registry = captureRegistry();
  const hash = hashOf(40);
  // feat/x 로 거르면 앞 다섯 행만 온다(행 40 은 빠진다).
  host.logRows = (params) =>
    (params.refs as string[] | null)?.includes("refs/heads/feat/x")
      ? (host.rows as LogRow[]).slice(0, 5)
      : undefined;
  await openLog();
  const branch = (name: string) => document.querySelector<HTMLElement>(`[data-ref="${name}"]`)!;
  await vi.waitFor(() => expect(branch("refs/heads/feat/x")).not.toBeNull());
  await mouse(branch("refs/heads/feat/x"), "click");
  expect(logCalls().at(-1)!.params.refs).toEqual(["refs/heads/feat/x"]);
  expect(branch("refs/heads/feat/x").getAttribute("aria-selected")).toBe("true");
  const before = logCalls().length;

  await act(async () => void registry()!.run("git.log.reveal", { rev: hash }));
  // 모든 ref 에서 닿는지 한 행으로 묻고, 닿으니 필터를 모두 풀고 다시 받는다.
  await vi.waitFor(() => expect(logCalls().length).toBe(before + 1));
  expect(host.calls.filter((c) => c.method === "git.logPage").at(-1)!.params).toMatchObject({
    refs: null,
    filter: { hash },
    limit: 1,
  });
  expect(logCalls().at(-1)!.params).toMatchObject({
    refs: null,
    filter: { author: null, message: null, since: null, until: null, paths: [], hash: null },
  });
  await vi.waitFor(() => expect(detailAsked(hash)).toBe(true));
  await vi.waitFor(() => expect(document.body.textContent).toContain("필터를 풀었다"));
  // 브랜치 패널도 「모든 브랜치」 로 돌아왔다.
  expect(branch("refs/heads/feat/x").getAttribute("aria-selected")).toBe("false");
});

it("git.log.reveal keeps the filter and warns when no ref reaches the commit", async () => {
  const registry = captureRegistry();
  const stray = "f".repeat(40);
  host.logRows = (params) =>
    (params.refs as string[] | null)?.includes("refs/heads/feat/x")
      ? (host.rows as LogRow[]).slice(0, 5)
      : undefined;
  await openLog();
  await mouse(document.querySelector<HTMLElement>('[data-ref="refs/heads/feat/x"]')!, "click");
  const before = logCalls().length;
  await act(async () => void registry()!.run("git.log.reveal", { rev: stray }));
  await vi.waitFor(() => expect(document.body.textContent).toContain("Git 로그에 없는 커밋이다"));
  expect(document.body.textContent).toContain("ffffff");
  // 거르기는 그대로다.
  expect(logCalls().length).toBe(before);
  expect(logCalls().at(-1)!.params.refs).toEqual(["refs/heads/feat/x"]);

  // 풀지 못하는 리비전은 오류 알림이고 아무것도 고르지 않는다.
  host.answers.set("git.show", { error: { code: -32602, message: "unknown revision: nope" } });
  await act(async () => void registry()!.run("git.log.reveal", { rev: "nope" }));
  await vi.waitFor(() => expect(document.body.textContent).toContain("unknown revision: nope"));
  expect(logCalls().length).toBe(before);
});
