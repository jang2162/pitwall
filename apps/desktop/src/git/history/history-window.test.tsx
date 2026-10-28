// @vitest-environment happy-dom
// 파일 히스토리(13-m2-plan M2-18 완료 기준)를 프로젝트 창(App kind="project")과 가짜 Host 로 본다. 가짜 Host 의 `git.fileHistory` ·
// `git.diff` · `git.blob` 답은 진짜 pw-git 의 출력이다(fixtures/history.json — crates/pw-git
// `file_history::tests::view_fixture` 가 이름이 두 번 바뀐 파일로 만들고, 그 JSON 이 지금 pw-git · git 의 출력과 같은지 본다). 목록은
// 같은 JSON 의 `git log --follow --name-status` 와, 칠한 줄은 그 커밋 · 경로의 `git show` patch 와 견준다.
//
// 여는 길은 셋이다: ⌃⌥H(트리에 포커스면 고른 파일, 아니면 활성 탭의 파일) · 트리 우클릭 · 탭 우클릭.

import { ActionRegistry } from "@pitwall/keymap";
import type {
  ConfigView,
  FileHistoryRow,
  FsEntry,
  GitBlob,
  GitDiff,
  HostInfo,
  KeymapFile,
  Project,
  ProjectTrust,
  Resolution,
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
import { diffPreferences } from "../documents/changes-view";
import { diffIn, paintedLines, parsePatch } from "../diff/testing";
import { resetWindowGitStatus } from "../git-status";
import fixture from "./fixtures/history.json";
import { ROW_HEIGHT } from "./history-document";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

interface Fixture {
  path: string;
  rows: FileHistoryRow[];
  /** `git log --follow --name-status`: (해시, 상태 글자, 경로들 — 이름 바뀜은 옛 경로가 앞). */
  follow: [string, string, string[]][];
  diffs: Record<string, GitDiff>;
  blobs: Record<string, GitBlob>;
  patches: Record<string, string>;
}

const FIXTURE = fixture as unknown as Fixture;
const FILE = `/repo/${FIXTURE.path}`;

const host = vi.hoisted(() => ({
  calls: [] as { method: string; params: Record<string, unknown> }[],
  /** `git.fileHistory` 가 보낼 행. */
  history: [] as unknown[],
  /** 있으면 `git.fileHistory` 가 행을 보낸 뒤 이 오류로 끝난다. */
  historyError: undefined as unknown,
  fixture: undefined as unknown as {
    diffs: Record<string, unknown>;
    blobs: Record<string, unknown>;
  },
  emit: (_topic: string, _payload: unknown) => {},
}));

vi.mock("../../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../../keymap/default-keymap");
  const { i18nState } = await import("../../i18n/testing");
  return {
    connectHost: async () => {
      const [client, server] = pair();
      const subs = new Map<number, string[]>();
      let next = 1;
      const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
      const project: Project = {
        id: "p1",
        name: "demo",
        host: "local",
        roots: ["/repo"],
        favorite: false,
        tags: [],
        configMode: "repository",
        slug: "demo",
        createdAt: 1,
        openedAt: null,
      };
      const entry = (name: string, kind: "file" | "dir"): FsEntry => ({
        name,
        kind,
        target: null,
        outside: false,
        size: 10,
        mtimeMs: 1,
        ignored: false,
      });
      const folders: Record<string, FsEntry[]> = {
        "/repo": [entry("dir", "dir"), entry("sub", "dir")],
        "/repo/dir": [entry("c.txt", "file")],
        // 서브모듈(작업 트리에 `.git` 파일).
        "/repo/sub": [entry(".git", "file"), entry("x.txt", "file")],
      };
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const reply = (result: unknown) => send({ id: m.id, result });
        const fail = (message: string) =>
          send({ id: m.id, error: { code: -32602, message, data: null } });
        const params = (m.params ?? {}) as Record<string, unknown>;
        host.calls.push({ method: m.method, params });
        const f = host.fixture;
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
              root: "/repo",
              path: "/repo",
              worktree: null,
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
          case "fs.list":
            return reply({ path: params.path, entries: folders[params.path as string] ?? [] });
          case "fs.stat":
            return params.path === "/repo/dir/c.txt"
              ? reply({
                  path: params.path,
                  kind: "file",
                  target: null,
                  size: 10,
                  mtimeMs: 1,
                  readonly: false,
                  sizeClass: "editable",
                })
              : fail(`not found: ${params.path as string}`);
          case "fs.read":
            return params.path === "/repo/dir/c.txt"
              ? reply({
                  path: params.path,
                  size: 10,
                  mtimeMs: 1,
                  offset: 0,
                  eof: true,
                  version: "v1",
                  content: { kind: "text", text: "tail added\n" },
                  judgment: {
                    binary: false,
                    encoding: "UTF-8",
                    lossy: false,
                    bom: null,
                    lineEnding: "lf",
                    longLines: false,
                    sizeClass: "editable",
                    editable: true,
                    highlight: false,
                  },
                })
              : fail(`not found: ${params.path as string}`);
          case "git.watch": {
            const streamId = next++;
            send({ id: m.id, result: { streamId } });
            return send({
              method: "stream.chunk",
              params: { streamId, data: { workTree: "/repo" } },
            });
          }
          case "git.status":
            return reply({ workTree: "/repo", entries: [], unsupported: null });
          case "git.repo":
            return reply({ repo: { workTree: "/repo" } });
          case "git.fileHistory": {
            const streamId = next++;
            send({ id: m.id, result: { streamId } });
            // pw-git 과 같은 청크: 첫 청크 200행, 그 뒤 500행.
            const rows = host.history;
            for (let at = 0; at < rows.length; at += at === 0 ? 200 : 500) {
              const data = { rows: rows.slice(at, at + (at === 0 ? 200 : 500)) };
              send({ method: "stream.chunk", params: { streamId, data } });
            }
            if (host.historyError)
              return send({
                method: "stream.end",
                params: { streamId, status: "failed", error: host.historyError },
              });
            return send({ method: "stream.end", params: { streamId, status: "completed" } });
          }
          case "git.diff": {
            const to = params.to as { rev: string };
            const diff = f.diffs[to.rev] as GitDiff | undefined;
            if (!diff)
              return reply({ workTree: "/repo", oldCommit: null, newCommit: to.rev, files: [] });
            const paths = params.paths as string[] | null | undefined;
            const files = paths ? diff.files.filter((x) => paths.includes(x.path)) : diff.files;
            return reply({ ...diff, files });
          }
          case "git.blob": {
            const blob = f.blobs[`${params.rev as string}:${params.path as string}`];
            return blob ? reply(blob) : fail(`no blob ${params.rev}:${params.path}`);
          }
          case "lang.detect": {
            const { files } = m.params as { files: { path: string }[] };
            return reply({
              files: files.map(() => ({
                language: null,
                source: "extension",
                vendored: false,
                generated: false,
                documentation: false,
              })),
            });
          }
          case "stream.cancel":
            return;
          case "events.subscribe": {
            const id = next++;
            subs.set(id, (m.params as { topics: string[] }).topics);
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
        for (const [subscriptionId, topics] of subs) {
          if (!topics.includes(topic)) continue;
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

/** 히스토리 목록의 보이는 높이: 10행. */
const LIST_HEIGHT = 10 * ROW_HEIGHT;
let root: Root | undefined;
let theme: ThemeStore | undefined;
let registries = new Set<ActionRegistry>();
const windowRegistry = () => [...registries].at(-1);
/** 다른 행의 액션(`git.log.reveal` · `git.showCommit`)을 부른 것 — 실행은 막고 인자만 남긴다. */
let ran: { id: string; args: unknown }[] = [];
const STUBBED = ["git.log.reveal", "git.showCommit"];

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  // TanStack Virtual 이 읽는 크기: 트리 600 px, 히스토리 목록 10행. 굴릴 수 있는 끝은 안쪽 요소의 높이.
  const height = (el: HTMLElement) =>
    el.getAttribute("role") === "tree"
      ? 600
      : el.hasAttribute("data-git-history-list")
        ? LIST_HEIGHT
        : 0;
  for (const prop of ["offsetHeight", "clientHeight"] as const)
    vi.spyOn(HTMLElement.prototype, prop, "get").mockImplementation(function (this: HTMLElement) {
      return height(this);
    });
  vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(function (
    this: HTMLElement,
  ) {
    const inner = height(this) > 0 ? this.firstElementChild : null;
    return inner instanceof HTMLElement ? Number.parseFloat(inner.style.height) || 0 : 0;
  });
  vi.spyOn(HTMLElement.prototype, "scrollTo").mockImplementation(function (
    this: HTMLElement,
    options?: ScrollToOptions | number,
  ) {
    if (typeof options === "object" && options.top !== undefined) this.scrollTop = options.top;
    this.dispatchEvent(new Event("scroll"));
  });
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.calls = [];
  host.history = FIXTURE.rows;
  host.historyError = undefined;
  host.fixture = FIXTURE;
  activeWorktreeStore.setState({ top: undefined });
  diffPreferences.setState({ mode: "split" });
  resetWindowFileTree();
  resetWindowGitStatus();
  localStorage.clear();
  const register = ActionRegistry.prototype.register;
  const seen = new Set<ActionRegistry>();
  registries = seen;
  vi.spyOn(ActionRegistry.prototype, "register").mockImplementation(function (
    this: ActionRegistry,
    action,
  ) {
    seen.add(this);
    return register.call(this, action);
  });
  ran = [];
  const realRun = ActionRegistry.prototype.run;
  vi.spyOn(ActionRegistry.prototype, "run").mockImplementation(function (
    this: ActionRegistry,
    id,
    args,
  ) {
    if (!STUBBED.includes(id)) return realRun.call(this, id, args);
    if (!this.isAvailable(id)) return false;
    ran.push({ id, args });
    return true;
  });
  const dispatch = document.dispatchEvent.bind(document);
  vi.spyOn(document, "dispatchEvent").mockImplementation((e) =>
    e.type === "selectionchange" ? true : dispatch(e),
  );
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
  await vi.waitFor(() => expect(windowRegistry()?.get("git.showFileHistory")).toBeDefined());
}

async function mouse(el: Element, type: "click" | "dblclick" | "contextmenu") {
  await act(async () => {
    const init = { bubbles: true, cancelable: true, button: type === "contextmenu" ? 2 : 0 };
    el.dispatchEvent(new PointerEvent("pointerdown", init));
    el.dispatchEvent(new MouseEvent("mousedown", init));
    if (type === "contextmenu") el.dispatchEvent(new MouseEvent("contextmenu", init));
    else {
      el.dispatchEvent(new MouseEvent("click", init));
      if (type === "dblclick") el.dispatchEvent(new MouseEvent("dblclick", init));
    }
  });
  await settle();
}

async function press(target: Element, init: KeyboardEventInit) {
  await act(async () => {
    target.dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
    );
    document.body.dispatchEvent(new KeyboardEvent("keyup", { bubbles: true, ...init }));
  });
  await settle();
}

const CTRL_ALT_H: KeyboardEventInit = { code: "KeyH", key: "˙", ctrlKey: true, altKey: true };

const treeRow = (path: string) =>
  document.querySelector<HTMLElement>(`[role=treeitem][data-path="${path}"]`);
const tabHeader = (text: string) =>
  [...document.querySelectorAll<HTMLElement>("[data-editor-tab-header]")].find((t) =>
    t.textContent?.includes(text),
  );
const historyRows = () => [...document.querySelectorAll<HTMLElement>("[data-git-history-row]")];
const historyRow = (hash: string) =>
  document.querySelector<HTMLElement>(`[data-git-history-row="${hash}"]`)!;
const status = () => document.querySelector("[data-git-history-status]")?.textContent;
const menuItem = (text: string) =>
  [...document.querySelectorAll<HTMLElement>("[role=menuitem]")].find(
    (e) => e.textContent === text,
  );
const fileHistoryCalls = () => host.calls.filter((c) => c.method === "git.fileHistory");

/** 트리에서 그 파일을 고른다(미리보기 탭이 열리고 포커스는 트리에 남는다). */
async function pickInTree() {
  await vi.waitFor(() => expect(treeRow("/repo/dir")).not.toBeNull());
  await mouse(treeRow("/repo/dir")!, "dblclick");
  await vi.waitFor(() => expect(treeRow(FILE)).not.toBeNull());
  await mouse(treeRow(FILE)!, "click");
  await vi.waitFor(() => expect(tabHeader("c.txt")).toBeDefined());
}

it("⌃⌥H in the tree follows the tree's selection, not the open tab: a chosen folder has no history", async () => {
  await mount();
  await pickInTree();
  // 미리보기 탭은 c.txt 인 채로 트리에서 폴더를 고른다.
  await mouse(treeRow("/repo/dir")!, "click");
  const tree = document.querySelector<HTMLElement>("[role=tree]")!;
  tree.focus();
  await press(tree, CTRL_ALT_H);
  await vi.waitFor(() =>
    expect(document.body.textContent).toContain("폴더의 히스토리는 볼 수 없다"),
  );
  expect(fileHistoryCalls()).toHaveLength(0);
  expect(tabHeader("c.txt 히스토리")).toBeUndefined();
});

async function openHistoryFromTree(count = FIXTURE.rows.length) {
  await mount();
  await pickInTree();
  const tree = document.querySelector<HTMLElement>("[role=tree]")!;
  tree.focus();
  await press(tree, CTRL_ALT_H);
  await vi.waitFor(() => expect(status()).toBe(`커밋 ${count.toLocaleString("ko")}개`));
}

it("⌃⌥H in the project tree opens the file's history: the commits of git log --follow, newest first, with the rename points", async () => {
  await openHistoryFromTree();
  expect(tabHeader("c.txt 히스토리")).toBeDefined();
  expect(fileHistoryCalls()).toHaveLength(1);
  expect(fileHistoryCalls()[0]!.params).toMatchObject({
    projectId: "p1",
    worktree: null,
    path: "dir/c.txt",
    rev: null,
  });

  // 목록이 git log --follow 와 같다: 커밋 · 차례 · 바뀜 글자 · 그때 경로.
  expect(FIXTURE.follow).toHaveLength(8);
  const shown = historyRows().map((r) => [
    r.dataset.gitHistoryRow,
    r.querySelector("[role=img]")!.textContent,
    r.dataset.path,
  ]);
  expect(shown).toEqual(
    FIXTURE.follow.map(([hash, letter, paths]) => [hash, letter, paths.at(-1)]),
  );

  // 이름 바뀐 지점: 두 커밋에 옛 → 새 이름, 그때 경로가 지금과 다른 행은 툴팁에 그 경로.
  const renames = historyRows()
    .map((r) => r.querySelector("[data-git-history-rename]")?.textContent)
    .filter(Boolean);
  expect(renames).toEqual(["이름 바뀜 b.txt → dir/c.txt", "이름 바뀜 a.txt → b.txt"]);
  const first = FIXTURE.follow.at(-1)![0];
  expect(historyRow(first).title).toBe("이 커밋에서의 경로: a.txt");
  expect(historyRow(FIXTURE.follow[0]![0]).title).toBe("");
  // 처음에는 맨 위 커밋을 고르고 그 diff 를 보인다.
  expect(historyRow(FIXTURE.follow[0]![0]).getAttribute("aria-selected")).toBe("true");
  // git 문서 탭은 파일 문서가 아니다 — 히스토리 탭의 키로 파일을 읽지 않는다.
  expect(
    host.calls.some(
      (c) =>
        (c.method === "fs.read" || c.method === "fs.stat") &&
        String(c.params.path).startsWith("git:"),
    ),
  ).toBe(false);
});

it("each commit's diff opens with the path of that commit, across both renames, and paints git show's lines", async () => {
  await openHistoryFromTree();
  for (const row of FIXTURE.rows) {
    await mouse(historyRow(row.hash), "click");
    const pane = () => document.querySelector(`[data-git-file-diff="${row.path}"]`);
    await vi.waitFor(() => expect(pane()).not.toBeNull());
    expect(document.querySelector(`[data-git-history-commit="${row.hash}"]`)).not.toBeNull();
    const call = host.calls.filter((c) => c.method === "git.diff").at(-1)!;
    expect({ hash: row.hash, ...call.params }).toMatchObject({
      hash: row.hash,
      to: { kind: "commit", rev: row.hash },
      paths: row.oldPath ? [row.path, row.oldPath] : [row.path],
    });
    expect(call.params.from).toBeUndefined();
    const want = parsePatch(FIXTURE.patches[row.hash]!).find((p) => p.path === row.path)!;
    expect(want, row.hash).toBeDefined();
    if (want.hunks.length === 0) {
      // 내용이 같은 이름 바뀜: 옛 이름을 보인다.
      expect(pane()!.textContent).toContain(row.oldPath!);
      continue;
    }
    await vi.waitFor(() => expect(diffIn(pane()!)).toBeDefined());
    expect({ hash: row.hash, ...paintedLines(pane()!, diffIn(pane()!)!) }).toEqual({
      hash: row.hash,
      deleted: want.deleted,
      added: want.added,
    });
  }
  // 더블클릭 · Enter 는 그 diff 를 탭으로 연다 — 이름이 바뀌기 전 커밋은 그때 경로로.
  const old = FIXTURE.rows.find((r) => r.path === "a.txt" && r.change === "modified")!;
  await mouse(historyRow(old.hash), "dblclick");
  await vi.waitFor(() => expect(tabHeader(`a.txt (${old.hash.slice(0, 7)})`)).toBeDefined());
  expect(host.calls.filter((c) => c.method === "git.diff").at(-1)!.params).toMatchObject({
    to: { kind: "commit", rev: old.hash },
    paths: ["a.txt"],
  });
});

it("⌃⌥H outside the tree uses the active file tab; the tab menu and the tree menu open the same tab", async () => {
  await mount();
  await pickInTree();
  // 트리 밖(에디터 · 창)에서는 활성 에디터 탭의 파일이다.
  (document.activeElement as HTMLElement | null)?.blur();
  await press(document.body, CTRL_ALT_H);
  await vi.waitFor(() => expect(tabHeader("c.txt 히스토리")).toBeDefined());
  expect(fileHistoryCalls().at(-1)!.params.path).toBe("dir/c.txt");
  const historyTabs = () =>
    [...document.querySelectorAll("[data-editor-tab-header]")].filter((t) =>
      t.textContent?.includes("히스토리"),
    );

  await mouse(tabHeader("c.txt")!, "contextmenu");
  await mouse(menuItem("히스토리 보기")!, "click");
  await settle();
  await mouse(treeRow(FILE)!, "contextmenu");
  await mouse(menuItem("히스토리 보기")!, "click");
  await settle();
  // 같은 파일이면 그 탭을 앞으로 — 새 탭을 만들지 않는다.
  expect(historyTabs()).toHaveLength(1);
  // 폴더의 메뉴 항목은 꺼져 있고, git 문서 탭의 메뉴에는 없다.
  await mouse(treeRow("/repo/dir")!, "contextmenu");
  expect(menuItem("히스토리 보기")!.hasAttribute("data-disabled")).toBe(true);
  await press(document.activeElement ?? document.body, { key: "Escape", code: "Escape" });
  await mouse(tabHeader("c.txt 히스토리")!, "contextmenu");
  expect(menuItem("히스토리 보기")).toBeUndefined();
  await press(document.activeElement ?? document.body, { key: "Escape", code: "Escape" });

  // git 문서 탭이 활성이면 볼 파일이 없다고 알린다.
  await mouse(tabHeader("c.txt 히스토리")!, "click");
  (document.activeElement as HTMLElement | null)?.blur();
  await press(document.body, CTRL_ALT_H);
  await vi.waitFor(() => expect(document.body.textContent).toContain("히스토리를 볼 파일이 없다"));
  expect(historyTabs()).toHaveLength(1);
});

it("a 1,000-commit history is virtualized: only the rows on screen exist and the list scrolls to the end", async () => {
  const many: FileHistoryRow[] = Array.from({ length: 1000 }, (_, i) => ({
    hash: i.toString(16).padStart(8, "0").padEnd(40, "a"),
    parents: i === 999 ? [] : [(i + 1).toString(16).padStart(8, "0").padEnd(40, "a")],
    subject: `change ${i}`,
    authorName: "Ada",
    authorEmail: "ada@example.com",
    authorTime: 1_700_000_000 - i * 60,
    path: i < 500 ? "dir/c.txt" : "old.txt",
    change: i === 999 ? "added" : i === 499 ? "renamed" : "modified",
    oldPath: i === 499 ? "old.txt" : null,
  }));
  host.history = many;
  await openHistoryFromTree(1000);
  const list = document.querySelector<HTMLElement>("[data-git-history-list]")!;
  // 그린 행은 화면(10행) + 위아래 덤뿐이다.
  expect(historyRows().length).toBeGreaterThan(0);
  expect(historyRows().length).toBeLessThanOrEqual(30);
  expect(Number.parseFloat((list.firstElementChild as HTMLElement).style.height)).toBe(
    1000 * ROW_HEIGHT,
  );

  // 가운데로 굴리면 그 언저리 행만 있다.
  await act(async () => {
    list.scrollTop = 600 * ROW_HEIGHT;
    list.dispatchEvent(new Event("scroll"));
  });
  await settle();
  const middle = historyRows().map((r) => r.textContent!);
  expect(middle.some((t) => t.includes("change 600"))).toBe(true);
  expect(middle.some((t) => t.includes("change 0"))).toBe(false);
  expect(historyRows().length).toBeLessThanOrEqual(30);

  // End 는 마지막 커밋을 고르고 화면 안으로 굴린다. 이름 바뀐 지점(행 499)도 표시가 있다.
  list.focus();
  await press(list, { key: "End", code: "End" });
  await vi.waitFor(() =>
    expect(historyRow(many[999]!.hash)?.getAttribute("aria-selected")).toBe("true"),
  );
  expect(list.scrollTop).toBeGreaterThanOrEqual(1000 * ROW_HEIGHT - LIST_HEIGHT - ROW_HEIGHT);
  await act(async () => {
    list.scrollTop = 495 * ROW_HEIGHT;
    list.dispatchEvent(new Event("scroll"));
  });
  await settle();
  expect(historyRow(many[499]!.hash).querySelector("[data-git-history-rename]")!.textContent).toBe(
    "이름 바뀜 old.txt → dir/c.txt",
  );
});

it("the row menu selects the commit in the Git log and opens its details by registry.run", async () => {
  await openHistoryFromTree();
  const row = FIXTURE.rows[3]!;
  await mouse(historyRow(row.hash), "contextmenu");
  await mouse(menuItem("Git 로그에서 선택")!, "click");
  expect(ran.at(-1)).toEqual({ id: "git.log.reveal", args: { rev: row.hash } });
  await mouse(historyRow(row.hash), "contextmenu");
  await mouse(menuItem("커밋 상세 열기")!, "click");
  expect(ran.at(-1)).toEqual({ id: "git.showCommit", args: { rev: row.hash } });
});

it("reloads when HEAD moves and keeps the chosen commit", async () => {
  await openHistoryFromTree();
  const row = FIXTURE.rows[2]!;
  await mouse(historyRow(row.hash), "click");
  await act(async () => host.emit("git.refsChanged", { worktree: null }));
  await vi.waitFor(() => expect(fileHistoryCalls()).toHaveLength(2));
  await settle();
  expect(historyRow(row.hash).getAttribute("aria-selected")).toBe("true");
  // 다른 워크트리의 바뀜은 무시한다.
  await act(async () => host.emit("git.refsChanged", { worktree: "/elsewhere" }));
  await settle();
  expect(fileHistoryCalls()).toHaveLength(2);
});

it("does not open for a path outside the repository and says so", async () => {
  await mount();
  await act(
    async () => void windowRegistry()!.run("git.showFileHistory", { path: "/elsewhere/x.txt" }),
  );
  await vi.waitFor(() => expect(document.body.textContent).toContain("저장소 밖의 파일이다"));
  expect(fileHistoryCalls()).toHaveLength(0);
  expect(document.querySelector("[data-git-history]")).toBeNull();
});

it("a file inside a submodule: the menus are off, ⌃⌥H says why, and a submodule the Host refuses shows why", async () => {
  await mount();
  await vi.waitFor(() => expect(treeRow("/repo/sub")).not.toBeNull());
  await mouse(treeRow("/repo/sub")!, "dblclick");
  await vi.waitFor(() => expect(treeRow("/repo/sub/x.txt")).not.toBeNull());
  await mouse(treeRow("/repo/sub/x.txt")!, "contextmenu");
  expect(menuItem("히스토리 보기")!.hasAttribute("data-disabled")).toBe(true);
  await press(document.activeElement ?? document.body, { key: "Escape", code: "Escape" });
  const tree = document.querySelector<HTMLElement>("[role=tree]")!;
  tree.focus();
  await press(tree, CTRL_ALT_H);
  await vi.waitFor(() =>
    expect(document.body.textContent).toContain("sub 는 따로 된 저장소(서브모듈 · 중첩 저장소)다"),
  );
  expect(fileHistoryCalls()).toHaveLength(0);

  // 트리가 펼친 적 없는 서브모듈이면 Host 가 거절하고(`data.submodule`), 탭이 빈 목록 대신 그 까닭을 보인다.
  host.history = [];
  host.historyError = {
    code: -32602,
    message: "invalid params",
    data: { detail: "`libs/sub/a.txt` is inside the submodule `libs/sub`", submodule: "libs/sub" },
  };
  await act(async () => void windowRegistry()!.run("git.showFileHistory", { path: FILE }));
  await vi.waitFor(() =>
    expect(status()).toBe(
      "libs/sub 서브모듈 안의 파일이다 — 히스토리는 그 서브모듈 저장소에서 본다",
    ),
  );
  expect(document.body.textContent).not.toContain("이 파일을 바꾼 커밋이 없다");
});

it("keeps the rows that came before a failure and shows the Host's reason, not just its code", async () => {
  host.history = FIXTURE.rows.slice(0, 2);
  host.historyError = {
    code: -32602,
    message: "invalid params",
    data: "the path `caf\uFFFD.txt` is not UTF-8 — it cannot be named again",
  };
  await mount();
  await act(async () => void windowRegistry()!.run("git.showFileHistory", { path: FILE }));
  await vi.waitFor(() => expect(status()).toContain("히스토리를 읽지 못했다: the path `caf"));
  expect(status()).toContain("is not UTF-8");
  expect(status()).not.toContain("-32602");
  expect(historyRows().map((r) => r.dataset.gitHistoryRow)).toEqual(
    FIXTURE.rows.slice(0, 2).map((r) => r.hash),
  );
});
