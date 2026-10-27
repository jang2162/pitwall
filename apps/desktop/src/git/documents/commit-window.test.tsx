// @vitest-environment happy-dom
// 커밋 상세 · 비교 · 리비전 파일 탭(13-m2-plan M2-15 완료 기준)을 프로젝트 창(App kind="project")과 가짜 Host 로 본다. 가짜 Host 의
// `git.show` · `git.diff` · `git.blob` 답은 진짜 pw-git 의 출력이다(src/git/diff/fixtures/commit.json — crates/pw-git
// `tests::diff::view_fixture` 가 만들고, 그 JSON 이 지금 pw-git · git 의 출력과 같은지 본다). 칠한 줄은 같은 JSON 의 `git show`
// patch 와 견준다.
//
// 액션은 다른 행이 부르는 길 그대로 레지스트리로 부른다(`registry.run("git.showCommit", { rev })`).

import { ActionRegistry } from "@pitwall/keymap";
import type {
  ConfigView,
  GitBlob,
  GitDiff,
  GitShow,
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
import { fakeTreeLayout } from "../../file-tree/testing";
import type { KeymapNative } from "../../keymap/native";
import { activeWorktreeStore } from "../../open-with/active-worktree";
import { documentThemeApplier } from "../../theme/adapters";
import { createThemeStore, type ThemeStore } from "../../theme/theme-store";
import { ThemeStoreProvider } from "../../theme/theme-switcher";
import { parseTheme } from "../../theme/tokens";
import { diffPreferences } from "./changes-view";
import fixture from "../diff/fixtures/commit.json";
import { diffIn, paintedLines, parsePatch } from "../diff/testing";
import { resetWindowGitStatus } from "../git-status";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const FIXTURE = fixture as unknown as {
  commit: string;
  show: GitShow;
  diff: GitDiff;
  blobs: Record<string, GitBlob>;
  patch: string;
};

const host = vi.hoisted(() => ({
  calls: [] as { method: string; params: Record<string, unknown> }[],
  /** 구문 세션(docId → 텍스트). */
  docs: new Map<string, string>(),
  fixture: undefined as unknown as {
    commit: string;
    show: GitShow;
    diff: GitDiff;
    blobs: Record<string, GitBlob>;
  },
}));

vi.mock("../../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../../keymap/default-keymap");
  const { i18nState } = await import("../../i18n/testing");
  return {
    connectHost: async () => {
      const [client, server] = pair();
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
            return reply({ path: params.path, entries: [] });
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
          case "git.show": {
            const rev = params.rev as string;
            if (rev === "HEAD" || rev === f.commit || rev === f.commit.slice(0, 7))
              return reply(f.show);
            if (rev === f.diff.oldCommit)
              return reply({ ...f.show, commit: rev, parents: [], refs: [], head: false });
            return fail(`unknown revision: ${rev}`);
          }
          case "git.diff": {
            const paths = params.paths as string[] | null | undefined;
            const files = paths ? f.diff.files.filter((x) => paths.includes(x.path)) : f.diff.files;
            return reply({ ...f.diff, files });
          }
          case "git.blob": {
            const blob = f.blobs[`${params.rev as string}:${params.path as string}`];
            return blob ? reply(blob) : fail(`no blob ${params.rev}:${params.path}`);
          }
          case "lang.detect": {
            const { files } = m.params as { files: { path: string }[] };
            return reply({
              files: files.map((x) => ({
                language: x.path.endsWith(".rs") ? "Rust" : null,
                source: "extension",
                vendored: false,
                generated: false,
                documentation: false,
              })),
            });
          }
          case "syntax.open": {
            host.docs.set(params.docId as string, params.text as string);
            return reply({ grammar: "rust", off: null });
          }
          case "syntax.highlights": {
            const text = host.docs.get(params.docId as string) ?? "";
            const spans = [...text.matchAll(/\bfn\b/g)].map((x) => ({
              from: x.index,
              to: x.index + 2,
              token: "keyword",
            }));
            return reply({ version: 0, spans, off: null });
          }
          case "syntax.close":
            host.docs.delete(params.docId as string);
            return reply(null);
          case "events.subscribe":
            return reply({ subscriptionId: next++ });
          case "events.unsubscribe":
            return reply(null);
          default:
            return send({ id: m.id, error: { code: -32601, message: `no ${m.method}` } });
        }
      });
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

let root: Root | undefined;
let theme: ThemeStore | undefined;
let registries = new Set<ActionRegistry>();
/** 이 창의 액션 레지스트리(마지막으로 등록한 인스턴스). */
const windowRegistry = () => [...registries].at(-1);

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  fakeTreeLayout(600);
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.calls = [];
  host.docs = new Map();
  host.fixture = FIXTURE;
  activeWorktreeStore.setState({ top: undefined });
  diffPreferences.setState({ mode: "split" });
  resetWindowFileTree();
  resetWindowGitStatus();
  localStorage.clear();
  // 창의 레지스트리를 잡는다 — 등록하는 인스턴스를 모은다(창마다 하나).
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
  // happy-dom 은 선택을 바꾸는 그 자리에서 `selectionchange` 를 동기로 보낸다(src/editor/editor-ops.test.tsx 와 같은 까닭).
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
  await vi.waitFor(() => expect(windowRegistry()?.get("git.showCommit")).toBeDefined());
}

async function click(el: Element) {
  await act(async () => {
    el.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, cancelable: true }));
    el.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
    el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
  });
  await settle();
}

const treeRow = (path: string) =>
  document.querySelector<HTMLElement>(`[data-git-change-path="${path}"]`);
const tabHeader = (text: string) =>
  [...document.querySelectorAll<HTMLElement>("[data-editor-tab-header]")].find((t) =>
    t.textContent?.includes(text),
  );
const short = FIXTURE.commit.slice(0, 7);
const PATCH = parsePatch(FIXTURE.patch);

async function openCommit() {
  await mount();
  let ran = false;
  await act(async () => {
    ran = windowRegistry()!.run("git.showCommit", { rev: "HEAD" });
  });
  expect(ran).toBe(true);
  await vi.waitFor(() => expect(tabHeader(short)).toBeDefined());
  await vi.waitFor(() => expect(treeRow("src/lib.rs")).not.toBeNull());
}

async function select(path: string) {
  await click(treeRow(path)!);
  await vi.waitFor(() =>
    expect(document.querySelector(`[data-git-file-diff="${path}"]`)).not.toBeNull(),
  );
}

async function painted(path: string) {
  const pane = () => document.querySelector(`[data-git-file-diff="${path}"]`)!;
  await vi.waitFor(() => expect(diffIn(pane())).toBeDefined());
  return paintedLines(pane(), diffIn(pane())!);
}

/** 내용 diff 가 있는(텍스트 · 생성 파일이 아님) 파일. */
const TEXT_FILES = FIXTURE.diff.files.filter(
  (f) => f.hunks.length > 0 && !f.generated && !f.binary,
);

it("git.showCommit 이 커밋 상세 탭을 열고, 파일마다 diff 가 git show 와 같은 줄을 칠한다(나란히 · 통합)", async () => {
  await openCommit();
  // 머리: 메시지 · 부모.
  const header = document.querySelector(`[data-git-commit-header="${FIXTURE.commit}"]`)!;
  expect(header.textContent).toContain("change things");
  expect(header.textContent).toContain("Body of the commit.");
  expect(header.querySelector(`[data-git-parent="${FIXTURE.show.parents[0]}"]`)).not.toBeNull();
  // 파일 트리에 바뀐 파일이 모두 있다.
  for (const f of FIXTURE.diff.files) expect(treeRow(f.path), f.path).not.toBeNull();

  expect(TEXT_FILES.map((f) => f.path)).toEqual(
    expect.arrayContaining([
      "src/lib.rs",
      "crlf.txt",
      "noeol.txt",
      "src/added.rs",
      "src/new_name.rs",
      "src/removed.rs",
    ]),
  );
  for (const mode of ["split", "unified"] as const) {
    act(() => diffPreferences.setState({ mode }));
    for (const f of TEXT_FILES) {
      await select(f.path);
      const want = PATCH.find((p) => p.path === f.path)!;
      const got = await painted(f.path);
      expect({ mode, path: f.path, ...got }).toEqual({
        mode,
        path: f.path,
        deleted: want.deleted,
        added: want.added,
      });
    }
  }
});

it("Cargo.lock(생성 파일) diff 는 접힌 채로 열리고 [펼치기]로 펼치면 git show 의 줄을 칠한다", async () => {
  await openCommit();
  // 첫 파일(경로순)이 Cargo.lock 이라 처음부터 고른 상태다.
  const pane = () => document.querySelector(`[data-git-file-diff="Cargo.lock"]`)!;
  await vi.waitFor(() => expect(pane()).not.toBeNull());
  expect(pane().textContent).toContain("생성 파일");
  expect(diffIn(pane())).toBeUndefined();
  expect(host.calls.some((c) => c.method === "git.blob" && c.params.path === "Cargo.lock")).toBe(
    false,
  );
  await click(pane().querySelector("[data-git-expand]")!);
  const want = PATCH.find((p) => p.path === "Cargo.lock")!;
  expect(await painted("Cargo.lock")).toEqual({ deleted: want.deleted, added: want.added });
  // 다른 파일을 갔다 와도 펼친 채다(문서마다 기억).
  await select("src/lib.rs");
  await select("Cargo.lock");
  expect(diffIn(pane())).toBeDefined();
});

it("바이너리 · 모드만 바뀐 파일 · 이름 바뀜은 그 까닭을 보이고, .rs 는 구문 강조(M1-17 의 구간)를 칠한다", async () => {
  await openCommit();
  await select("bin.dat");
  expect(document.querySelector(`[data-git-file-diff="bin.dat"]`)!.textContent).toContain(
    "바이너리",
  );
  await select("script.sh");
  const script = document.querySelector(`[data-git-file-diff="script.sh"]`)!;
  expect(script.textContent).toContain("100644 → 100755");
  expect(script.textContent).toContain("내용은 그대로다");
  await select("src/new_name.rs");
  expect(document.querySelector(`[data-git-file-diff="src/new_name.rs"]`)!.textContent).toContain(
    "src/old_name.rs",
  );
  await select("src/lib.rs");
  const pane = document.querySelector(`[data-git-file-diff="src/lib.rs"]`)!;
  await vi.waitFor(() =>
    expect(pane.querySelectorAll(".pw-syntax-keyword").length).toBeGreaterThan(0),
  );
});

it("부모 해시를 누르면 그 커밋 상세가 새 탭으로 열리고, 같은 커밋을 다시 열면 탭을 새로 만들지 않는다", async () => {
  await openCommit();
  await act(async () => void windowRegistry()!.run("git.showCommit", { rev: FIXTURE.commit }));
  await settle();
  expect(document.querySelectorAll("[data-editor-tab-header]")).toHaveLength(1);
  const parent = FIXTURE.show.parents[0]!;
  await click(document.querySelector(`[data-git-parent="${parent}"]`)!);
  await vi.waitFor(() => expect(tabHeader(parent.slice(0, 7))).toBeDefined());
  expect(document.querySelectorAll("[data-editor-tab-header]")).toHaveLength(2);
});

it("인자가 없거나 틀린 git 문서 액션은 아무것도 열지 않는다 — 키 · 액션 찾기에서 부를 수 없다", async () => {
  await mount();
  for (const id of ["git.showCommit", "git.compareCommits", "git.openRevisionFile"]) {
    expect(windowRegistry()!.isEnabled(id)).toBe(false);
    expect(windowRegistry()!.run(id)).toBe(false);
  }
  await act(async () => void windowRegistry()!.run("git.showCommit", { rev: "" }));
  await act(
    async () => void windowRegistry()!.run("git.openRevisionFile", { rev: "HEAD", path: "/abs" }),
  );
  await act(async () => void windowRegistry()!.run("git.compareCommits", { a: "HEAD" }));
  await settle();
  expect(document.querySelectorAll("[data-editor-tab-header]")).toHaveLength(0);
  expect(host.calls.some((c) => c.method === "git.show")).toBe(false);
});

it("git.compareCommits 가 두 커밋 비교 탭을, git.openRevisionFile 이 리비전 표시가 있는 읽기 전용 탭을 연다", async () => {
  await mount();
  const parent = FIXTURE.show.parents[0]!;
  await act(async () => void windowRegistry()!.run("git.compareCommits", { a: parent, b: "HEAD" }));
  await vi.waitFor(() => expect(treeRow("src/lib.rs")).not.toBeNull());
  const diffCall = host.calls.find((c) => c.method === "git.diff")!;
  expect(diffCall.params.from).toEqual({ kind: "commit", rev: parent });
  expect(diffCall.params.to).toEqual({ kind: "commit", rev: FIXTURE.commit });

  await act(
    async () =>
      void windowRegistry()!.run("git.openRevisionFile", { rev: "HEAD", path: "src/lib.rs" }),
  );
  await vi.waitFor(() =>
    expect(document.querySelector(`[data-git-revision-bar="${FIXTURE.commit}"]`)).not.toBeNull(),
  );
  const bar = document.querySelector(`[data-git-revision-bar="${FIXTURE.commit}"]`)!;
  expect(bar.textContent).toContain(short);
  expect(bar.textContent).toContain("읽기 전용");
  await vi.waitFor(() =>
    expect(document.querySelector("[data-git-revision-text] .cm-content")).not.toBeNull(),
  );
  const text = document.querySelector("[data-git-revision-text] .cm-content")!.textContent!;
  expect(text).toContain("fn uno() {");
  // 탭 묶음(다시 시작해 되살리는 것)에 git 문서 탭이 들지 않는다 — 파일 읽기(`fs.read` · `fs.stat`)도 하지 않는다.
  expect(host.calls.some((c) => c.method === "fs.stat" || c.method === "fs.read")).toBe(false);
});
