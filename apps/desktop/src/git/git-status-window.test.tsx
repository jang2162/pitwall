// @vitest-environment happy-dom
// 프로젝트 창(App kind="project")의 git 상태 색(13-m2-plan M2-12 완료 기준)을 가짜 Host 에 붙여 본다: 트리의 글자 · 색이 진짜
// `git status --porcelain` 의 줄과 같고, `git.statusChanged` 뒤 트리와 탭 색이 바뀌고, 라이트 ↔ 다크 전환을 따르고, 창이 닫히거나 활성
// 워크트리가 바뀌면 `git.watch` 를 놓는다.

import type {
  ConfigView,
  FsEntry,
  FsRead,
  FsStat,
  GitStatus,
  HostInfo,
  KeymapFile,
  LangDetectResult,
  Project,
  ProjectTrust,
  Resolution,
} from "@pitwall/rpc-client";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import { resetWindowFileTree } from "../file-tree/project-view";
import { fakeTreeLayout } from "../file-tree/testing";
import type { KeymapNative } from "../keymap/native";
import { activeWorktreeStore, rememberActiveWorktree } from "../open-with/active-worktree";
import { projectOpenInbox } from "../project-open";
import { documentThemeApplier } from "../theme/adapters";
import { createThemeStore, type ThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { type GitStatusChanged, resetWindowGitStatus, windowGitStatus } from "./git-status";
import { EXPECTED_LETTERS, PORCELAIN, porcelainEntries } from "./testing";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const host = vi.hoisted(() => ({
  /** 폴더 경로 → 이름 → 종류. */
  dirs: new Map<string, Map<string, { kind: "file" | "dir"; ignored?: boolean }>>(),
  /** 작업 트리(워크트리 꼭대기 → 그 status). 없는 워크트리는 저장소가 없다. */
  repos: new Map<string, GitStatus>(),
  calls: [] as { method: string; params: Record<string, unknown> }[],
  watches: [] as { streamId: number; worktree: string | null }[],
  cancelled: [] as number[],
  closed: false,
  emit: (_topic: string, _payload: unknown) => {},
}));

vi.mock("../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../keymap/default-keymap");
  const { i18nState } = await import("../i18n/testing");
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
        const fail = (message: string) =>
          send({ id: m.id, error: { code: -32004, message, data: null } });
        const params = (m.params ?? {}) as Record<string, unknown>;
        if (m.method.startsWith("git.")) host.calls.push({ method: m.method, params });
        const path = (params.path as string | undefined) ?? "";
        const slash = path.lastIndexOf("/");
        const self = host.dirs.get(path.slice(0, slash))?.get(path.slice(slash + 1));
        const top = (params.worktree as string | null | undefined) ?? "/work/demo";
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
          case "project.resolve":
            return reply({
              project,
              root: "/work/demo",
              path,
              worktree: { top: path, root: path },
            } satisfies Resolution);
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
          case "fs.list": {
            const dir = host.dirs.get(path);
            if (!dir) return fail(`not a folder: ${path}`);
            const entries: FsEntry[] = [...dir.entries()]
              .sort(([a], [b]) => a.localeCompare(b))
              .map(([name, e]) => ({
                name,
                kind: e.kind,
                target: null,
                outside: false,
                size: 0,
                mtimeMs: 1,
                ignored: e.ignored ?? false,
              }));
            return reply({ path, entries });
          }
          case "fs.stat":
            if (!self) return fail(`not found: ${path}`);
            return reply({
              path,
              kind: self.kind,
              target: null,
              size: path.length,
              mtimeMs: 1,
              readonly: false,
              sizeClass: "editable",
            } satisfies FsStat);
          case "fs.read":
            return reply({
              path,
              size: path.length,
              mtimeMs: 1,
              offset: 0,
              eof: true,
              version: "v1",
              content: { kind: "text", text: path },
              judgment: {
                binary: false,
                encoding: "UTF-8",
                lossy: false,
                bom: null,
                lineEnding: "lf",
                longLines: false,
                sizeClass: "editable",
                editable: true,
                highlight: true,
              },
            } satisfies FsRead);
          case "lang.detect": {
            const { files } = m.params as { files: { path: string }[] };
            return reply({
              files: files.map(() => ({
                language: null,
                source: "none",
                vendored: false,
                generated: false,
                documentation: false,
              })),
            } satisfies LangDetectResult);
          }
          case "git.watch": {
            const streamId = next++;
            host.watches.push({ streamId, worktree: (params.worktree as string | null) ?? null });
            send({ id: m.id, result: { streamId } });
            const workTree = host.repos.has(top) ? top : null;
            return send({ method: "stream.chunk", params: { streamId, data: { workTree } } });
          }
          case "stream.cancel":
            return host.cancelled.push(params.streamId as number);
          case "git.status": {
            const repo = host.repos.get(top);
            if (!repo) return reply({ workTree: null, entries: [], unsupported: null });
            const paths = params.paths as string[] | null;
            const inside = (p: string) =>
              !paths || paths.some((q) => p === q || p.startsWith(`${q}/`));
            return reply({ ...repo, entries: repo.entries.filter((e) => inside(e.path)) });
          }
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
      return {
        transport: client,
        close: async () => {
          host.closed = true;
        },
      };
    },
  };
});

const native: KeymapNative = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
  closeWindow: async () => {},
};

/** `paths` 의 파일(끝이 `/` 면 폴더)을 디스크에 만든다. */
function disk(paths: string[], ignored: string[] = []) {
  for (const p of paths) {
    const parts = p.replace(/\/$/, "").split("/");
    for (let i = 2; i <= parts.length; i++) {
      const parent = parts.slice(0, i - 1).join("/");
      const name = parts[i - 1]!;
      const isLeafFile = i === parts.length && !p.endsWith("/");
      if (!host.dirs.has(parent)) host.dirs.set(parent, new Map());
      const path = `${parent}/${name}`;
      host.dirs.get(parent)!.set(name, {
        kind: isLeafFile ? "file" : "dir",
        ignored: ignored.some((i) => path === i || path.startsWith(`${i}/`)),
      });
      if (!isLeafFile && !host.dirs.has(path)) host.dirs.set(path, new Map());
    }
  }
}

const REPO = "/work/demo";
/** porcelain 고정물의 항목 중 디스크에 있는 것(지운 파일은 트리에 없다). */
const ON_DISK = Object.keys(EXPECTED_LETTERS).filter(
  (p) =>
    ![
      "src/deleted.rs",
      "src/staged-deleted.rs",
      "src/mod-deleted.rs",
      "src/added-deleted.rs",
    ].includes(p),
);

let root: Root | undefined;
let theme: ThemeStore | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  fakeTreeLayout(600);
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.dirs = new Map();
  host.calls = [];
  host.watches = [];
  host.cancelled = [];
  host.closed = false;
  host.repos = new Map([
    [REPO, { workTree: REPO, entries: porcelainEntries(PORCELAIN), unsupported: null }],
  ]);
  disk(
    [
      ...ON_DISK.map((p) => `${REPO}/${p}`),
      `${REPO}/docs/readme.md`,
      `${REPO}/target/debug/app`,
      "/work/wt/clean.txt",
    ],
    [`${REPO}/target`],
  );
  activeWorktreeStore.setState({ top: undefined });
  resetWindowFileTree();
  resetWindowGitStatus();
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

const settle = (ms = 20) => act(async () => new Promise((r) => setTimeout(r, ms)));

async function mount() {
  const element = document.createElement("div");
  element.style.width = "1280px";
  element.style.height = "800px";
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

const row = (path: string) => document.querySelector<HTMLElement>(`[data-path="${path}"]`);
const letterOf = (el: HTMLElement | null) =>
  el?.querySelector<HTMLElement>("[data-git-letter]")?.textContent ?? null;
const toneOf = (el: HTMLElement | null) => el?.getAttribute("data-git") ?? null;

async function click(el: HTMLElement, type: "click" | "dblclick" = "click") {
  await act(async () => {
    el.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true }));
  });
  await settle();
}

async function expandAll() {
  for (const dir of [`${REPO}/src`, `${REPO}/new`, `${REPO}/docs`]) {
    await vi.waitFor(() => expect(row(dir)).not.toBeNull());
    if (row(dir)!.getAttribute("aria-expanded") !== "true") await click(row(dir)!, "dblclick");
  }
  await vi.waitFor(() => expect(row(`${REPO}/src/modified.rs`)).not.toBeNull());
}

const tab = (name: string) =>
  [...document.querySelectorAll<HTMLElement>("[data-editor-tab-header]")].find((t) =>
    t.textContent?.includes(name),
  );

it("shows each file's letter and color as git status --porcelain says", async () => {
  await mount();
  await expandAll();
  await vi.waitFor(() => expect(letterOf(row(`${REPO}/src/modified.rs`))).toBe("M"));
  const shown = Object.fromEntries(ON_DISK.map((p) => [p, letterOf(row(`${REPO}/${p}`))]));
  expect(shown).toEqual(Object.fromEntries(ON_DISK.map((p) => [p, EXPECTED_LETTERS[p]])));

  const conflict = row(`${REPO}/src/conflict.rs`)!;
  expect(toneOf(conflict)).toBe("conflicted");
  expect(conflict.className).toContain("text-git-conflicted");
  expect(conflict.title).toBe("충돌: 양쪽에서 고침");
  // 스크린 리더는 글자 대신 상태 이름을 읽는다.
  expect(conflict.querySelector("[data-git-letter]")!.getAttribute("aria-label")).toBe("충돌");
  expect(row(`${REPO}/src/both.rs`)!.title).toBe("스테이징됨: 수정됨 · 작업 트리: 수정됨");
  expect(toneOf(row(`${REPO}/src/new-name.rs`))).toBe("renamed");
  expect(toneOf(row(`${REPO}/untracked.txt`))).toBe("untracked");

  // 폴더는 색만 — 아래의 충돌이 이긴다. 전부 untracked 인 폴더는 untracked. 바뀐 것이 없는 폴더 · 파일은 칠하지 않는다.
  expect(toneOf(row(`${REPO}/src`))).toBe("conflicted");
  expect(letterOf(row(`${REPO}/src`))).toBeNull();
  expect(toneOf(row(`${REPO}/new`))).toBe("untracked");
  expect(toneOf(row(`${REPO}/docs`))).toBeNull();
  // ignore 된 폴더는 git.ignored 색에 기울임이다.
  expect(row(`${REPO}/target`)!.className).toContain("text-git-ignored");
  expect(row(`${REPO}/target`)!.querySelector(".italic")).not.toBeNull();

  // 전체를 한 번 읽었다.
  expect(host.calls.filter((c) => c.method === "git.status").map((c) => c.params.paths)).toEqual([
    null,
  ]);
});

it("colors an editor tab's title and puts the letter before its name", async () => {
  await mount();
  await act(async () => projectOpenInbox.push({ file: `${REPO}/src/added.rs` }));
  await vi.waitFor(() => expect(tab("added.rs")).toBeDefined());
  await vi.waitFor(() => expect(toneOf(tab("added.rs")!)).toBe("added"));
  const header = tab("added.rs")!;
  expect(header.className).toContain("text-git-added");
  expect(letterOf(header)).toBe("A");
  expect(header.querySelector("[data-git-letter]")!.getAttribute("aria-label")).toBe("추가됨");
  expect(header.title).toBe(`${REPO}/src/added.rs\n스테이징됨: 추가됨`);
  // 글자가 이름 앞이다.
  expect(header.textContent).toBe("Aadded.rs");
});

// 완료 기준은 「터미널에서 파일을 고치면 500 ms 안에 트리와 탭 색이 바뀐다」. Host 의 감시가 `git.statusChanged` 를 내기까지 500 ms
// 안이다(crates/pw-git/src/tests/watch.rs, release 에서 단언 — 잰 값은 보고서). 창 쪽은 시간이 아니라 일로 본다 — 이벤트 하나에
// 바뀐 경로만 담은 `git.status` 한 번, 그 응답으로 행 · 탭이 바뀐다. 끝에서 끝까지의 실제 시간은 13-m2-plan.md 「M2 에서 하지 않는
// 것」 의 M2-12 후속 행.
it("recolors the tree and the tab from one git.status of the changed paths after git.statusChanged", async () => {
  await mount();
  await expandAll();
  await act(async () => projectOpenInbox.push({ file: `${REPO}/docs/readme.md` }));
  await vi.waitFor(() => expect(tab("readme.md")).toBeDefined());
  await vi.waitFor(() => expect(toneOf(row(`${REPO}/src`))).toBe("conflicted"));
  expect(toneOf(tab("readme.md")!)).toBeNull();
  expect(toneOf(row(`${REPO}/docs`))).toBeNull();

  // 터미널에서 docs/readme.md 를 고치고, 충돌을 풀어 스테이징했다.
  const repo = host.repos.get(REPO)!;
  host.repos.set(REPO, {
    ...repo,
    entries: [
      ...repo.entries.filter((e) => e.path !== "src/conflict.rs"),
      {
        path: "docs/readme.md",
        origPath: null,
        staged: null,
        unstaged: "modified",
        conflict: null,
      },
      {
        path: "src/conflict.rs",
        origPath: null,
        staged: "modified",
        unstaged: null,
        conflict: null,
      },
    ],
  });
  const reads = host.calls.filter((c) => c.method === "git.status").length;
  await act(async () =>
    host.emit("git.statusChanged", {
      worktree: null,
      paths: ["docs/readme.md", "src/conflict.rs"],
    } satisfies GitStatusChanged),
  );
  await vi.waitFor(() => {
    expect(toneOf(tab("readme.md")!)).toBe("modified");
    expect(letterOf(row(`${REPO}/docs/readme.md`))).toBe("M");
    expect(letterOf(row(`${REPO}/src/conflict.rs`))).toBe("M");
  });
  expect(host.calls.filter((c) => c.method === "git.status")).toHaveLength(reads + 1);
  expect(toneOf(row(`${REPO}/docs`))).toBe("modified");
  expect(toneOf(row(`${REPO}/src`))).toBe("modified");
  expect(host.calls.filter((c) => c.method === "git.status").at(-1)!.params.paths).toEqual([
    "docs/readme.md",
    "src/conflict.rs",
  ]);
});

it("follows a light ↔ dark switch through the git token variables", async () => {
  await mount();
  await expandAll();
  await vi.waitFor(() => expect(toneOf(row(`${REPO}/src/modified.rs`))).toBe("modified"));
  const value = () =>
    document.documentElement.style.getPropertyValue("--pw-git-modified").toLowerCase();
  expect(row(`${REPO}/src/modified.rs`)!.className).toContain("text-git-modified");
  expect(value()).toBe(defaultTheme.variants.light["git.modified"]);
  await act(async () => theme!.getState().setPreference("dark"));
  expect(value()).toBe(defaultTheme.variants.dark["git.modified"]);
  expect(document.documentElement.dataset.theme).toBe("dark");
  await act(async () => theme!.getState().setPreference("light"));
  expect(value()).toBe(defaultTheme.variants.light["git.modified"]);
});

it("lets go of git.watch when the active worktree changes and when the window closes", async () => {
  await mount();
  await expandAll();
  await vi.waitFor(() => expect(letterOf(row(`${REPO}/src/modified.rs`))).toBe("M"));
  expect(host.watches).toEqual([{ streamId: expect.any(Number), worktree: null }]);
  const first = host.watches[0]!.streamId;

  // 저장소가 없는 워크트리로 옮긴다: 옛 스트림을 닫고 새 워크트리로 연다. 칠할 것이 없다.
  await act(async () => rememberActiveWorktree({ worktree: "/work/wt" }));
  await vi.waitFor(() => expect(row("/work/wt/clean.txt")).not.toBeNull());
  await vi.waitFor(() => expect(host.cancelled).toEqual([first]));
  expect(host.watches.map((w) => w.worktree)).toEqual([null, "/work/wt"]);
  expect(toneOf(row("/work/wt/clean.txt"))).toBeNull();
  // 다른 워크트리의 이벤트는 읽지 않는다.
  const reads = host.calls.filter((c) => c.method === "git.status").length;
  await act(async () =>
    host.emit("git.statusChanged", { worktree: null, paths: ["src/modified.rs"] }),
  );
  await settle();
  expect(host.calls.filter((c) => c.method === "git.status")).toHaveLength(reads);

  // 창이 닫히면 Host 연결이 닫히고 Host 는 그 연결의 스트림을 놓는다(fs.watch 와 같은 길). 창의 status 도 비운다.
  expect(windowGitStatus().workTree()).toBeNull();
  await act(async () => rememberActiveWorktree({ file: `${REPO}/README` }));
  await vi.waitFor(() => expect(windowGitStatus().workTree()).toBe(REPO));
  act(() => root!.unmount());
  root = undefined;
  await vi.waitFor(() => expect(host.closed).toBe(true));
  expect(windowGitStatus().workTree()).toBeNull();
});

it("paints nothing for a repository gix cannot read (reftable)", async () => {
  host.repos.set(REPO, { workTree: REPO, entries: [], unsupported: "reftable" });
  await mount();
  await expandAll();
  await vi.waitFor(() => expect(host.calls.some((c) => c.method === "git.status")).toBe(true));
  await settle();
  expect(document.querySelectorAll("[data-git]")).toHaveLength(0);
});
