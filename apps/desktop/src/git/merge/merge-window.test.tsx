// @vitest-environment happy-dom
// 충돌 해결(13-m2-plan M2-19 완료 기준)을 프로젝트 창(App kind="project")과 가짜 Host 로 본다. 가짜 Host 의 `git.conflicts` ·
// `git.conflict.get` · rebase 의 `git.operation` 답은 진짜 pw-git 의 출력이다(fixtures/merge.json — crates/pw-git
// `conflict::tests::view_fixture` 가 만들고, 그 JSON 이 지금 pw-git 의 출력과 같은지, 그리고 창이 저장할 결과(`expected`)를 같은
// 저장소에 풀고 계속하면 머지 커밋이 그 바이트로 생기는지 본다). 이 시험은 창이 그 `expected` 를 보내는지와 [계속] · [중단] 이 맞는
// RPC 를 부르는지 본다 — 둘을 이어 「창에서 커밋까지」다.

import { EditorView } from "@codemirror/view";
import { ActionRegistry } from "@pitwall/keymap";
import type {
  ConfigView,
  GitConflictDetail,
  GitConflicts,
  GitOperationResult,
  GitRefs,
  HostInfo,
  KeymapFile,
  OperationInfo,
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
import { resetWindowBranch } from "../branch-state";
import { resetWindowGitStatus } from "../git-status";
import fixture from "./fixtures/merge.json";
import { mergeField } from "./merge-model";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const FIXTURE = fixture as unknown as {
  conflicts: GitConflicts;
  detail: GitConflictDetail;
  image: GitConflictDetail;
  typed: string;
  expected: string;
  rebase: { operation: OperationInfo; abort: GitOperationResult };
};

const MERGING: OperationInfo = {
  kind: "merge",
  heads: ["9d6fea61c3f1ac7d1499e2aea828eae257d0853e"],
  branch: null,
  onto: null,
  step: null,
  remaining: null,
  actions: ["continue", "abort"],
};

const host = vi.hoisted(() => ({
  calls: [] as { method: string; params: Record<string, unknown> }[],
  operation: null as OperationInfo | null,
  conflicts: undefined as unknown as GitConflicts,
  /** 경로 → get 의 답. */
  details: new Map<string, GitConflictDetail>(),
  /** 다음 `git.conflict.resolve` 를 이 오류로 거절한다. */
  refuseResolve: undefined as { code: number; message: string } | undefined,
  continueResult: undefined as unknown,
  abortResult: undefined as unknown,
  emit: (_topic: string, _payload: unknown) => {},
}));

vi.mock("../../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../../keymap/default-keymap");
  const { i18nState } = await import("../../i18n/testing");
  return {
    connectHost: async () => {
      const [client, server] = pair();
      let next = 1;
      const subs = new Map<number, { projectId?: string; topics: string[] }>();
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
      const refs: GitRefs = {
        workTree: "/repo",
        head: { kind: "branch", name: "main", commit: "35022f5702b3202fbbf07fd1528e6076317ab109" },
        branches: [],
        remoteBranches: [],
        tags: [],
        unsupported: null,
        broken: [],
      };
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const reply = (result: unknown) => send({ id: m.id, result });
        const fail = (code: number, message: string) =>
          send({ id: m.id, error: { code, message, data: null } });
        const params = (m.params ?? {}) as Record<string, unknown>;
        host.calls.push({ method: m.method, params });
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
          case "stream.cancel":
            return;
          case "git.status":
            return reply({
              workTree: "/repo",
              entries: host.conflicts.files.map((f) => ({
                path: f.path,
                origPath: null,
                staged: null,
                unstaged: null,
                conflict: f.kind,
              })),
              unsupported: null,
            });
          case "git.refs":
            return reply(refs);
          case "git.operation":
            return reply({ operation: host.operation });
          case "git.repo":
            return reply({ repo: { workTree: "/repo" } });
          case "git.conflicts":
            return reply(host.conflicts);
          case "git.conflict.get": {
            const d = host.details.get(params.path as string);
            return d ? reply(d) : fail(-32005, "not in conflict");
          }
          case "git.conflict.resolve": {
            if (host.refuseResolve) {
              const e = host.refuseResolve;
              host.refuseResolve = undefined;
              return fail(e.code, e.message);
            }
            const path = params.path as string;
            host.conflicts = {
              ...host.conflicts,
              files: host.conflicts.files.filter((f) => f.path !== path),
            };
            host.details.delete(path);
            return reply(host.conflicts);
          }
          case "git.operation.continue":
            return reply(host.continueResult);
          case "git.operation.abort":
            return reply(host.abortResult);
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
          case "events.subscribe": {
            const id = next++;
            subs.set(id, m.params as { projectId?: string; topics: string[] });
            return reply({ subscriptionId: id });
          }
          case "events.unsubscribe":
            return reply(null);
          default:
            return m.id === undefined
              ? undefined
              : send({ id: m.id, error: { code: -32601, message: `no ${m.method}` } });
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

let root: Root | undefined;
let theme: ThemeStore | undefined;
let registries = new Set<ActionRegistry>();
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
  host.operation = MERGING;
  host.conflicts = structuredClone(FIXTURE.conflicts);
  host.details = new Map([
    ["notes.txt", structuredClone(FIXTURE.detail)],
    ["image.bin", structuredClone(FIXTURE.image)],
  ]);
  host.refuseResolve = undefined;
  host.continueResult = { operation: null, stopped: false, moved: null };
  host.abortResult = FIXTURE.rebase.abort;
  activeWorktreeStore.setState({ top: undefined });
  resetWindowFileTree();
  resetWindowGitStatus();
  resetWindowBranch();
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
  await vi.waitFor(() => expect(windowRegistry()?.get("git.openMerge")).toBeDefined());
}

async function click(el: Element) {
  await act(async () => {
    el.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, cancelable: true }));
    el.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
    el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
  });
  await settle();
}

const banner = () => document.querySelector<HTMLElement>("[data-testid=git-operation-banner]");
const bannerButton = (what: string) =>
  banner()?.querySelector<HTMLElement>(`[data-git-operation="${what}"]`) ?? null;
const conflictRow = (path: string) =>
  document.querySelector<HTMLElement>(`[data-git-conflict="${path}"]`);
const pane = (p: "ours" | "result" | "theirs") =>
  EditorView.findFromDOM(
    document.querySelector<HTMLElement>(`[data-git-merge-pane="${p}"] .cm-editor`)!,
  )!;
const regionButton = (p: "ours" | "theirs", index: number, action: "apply" | "ignore") =>
  document.querySelector<HTMLElement>(
    `[data-git-merge-pane="${p}"] [data-merge-region="${index}"][data-merge-action="${action}"]`,
  );
const unresolvedText = () =>
  document.querySelector("[data-testid=git-merge-unresolved]")?.textContent;
const calls = (method: string) =>
  host.calls.filter((c) => c.method === method).map((c) => c.params);
const confirmDialog = () =>
  document.querySelector<HTMLElement>("[data-testid=file-confirm-dialog]");
const toasts = () => document.body.textContent ?? "";

async function openMerge(path = "notes.txt") {
  await act(async () => {
    windowRegistry()!.run("git.openMerge", { path });
  });
  await vi.waitFor(() =>
    expect(
      document.querySelector(
        `[data-git-merge="${path}"] [data-git-merge-pane="result"] .cm-editor`,
      ),
    ).not.toBeNull(),
  );
  await settle();
}

/** 결과 칸의 i 번째 구간 위치. */
const rangeOf = (i: number) => pane("result").state.field(mergeField).ranges[i]!;

it("resolves the merge fixture in the window: pick a side, ≫ one conflict, type the other, save, continue", async () => {
  await mount();
  await vi.waitFor(() => expect(banner()?.dataset.operation).toBe("merge"));
  expect(banner()!.textContent).toContain("머지 진행 중");
  await vi.waitFor(() =>
    expect(document.querySelector("[data-testid=git-operation-conflicts]")?.textContent).toBe(
      "충돌 파일 2개",
    ),
  );
  // 머지에는 건너뛰기가 없다.
  expect(bannerButton("skip")).toBeNull();

  // 충돌 목록: 바이너리는 [병합…] 이 꺼져 있다.
  await click(bannerButton("resolve")!);
  await vi.waitFor(() => expect(conflictRow("image.bin")).not.toBeNull());
  const mergeImage =
    conflictRow("image.bin")!.querySelector<HTMLButtonElement>("[data-git-merge-open]")!;
  expect(mergeImage.disabled).toBe(true);
  expect(mergeImage.title).toContain("바이너리");
  expect(
    conflictRow("notes.txt")!.querySelector<HTMLButtonElement>("[data-git-merge-open]")!.disabled,
  ).toBe(false);

  // [내 것]: 먼저 get 해서 그 version 을 싣는다.
  await click(conflictRow("image.bin")!.querySelector("[data-git-pick=ours]")!);
  await vi.waitFor(() => expect(conflictRow("image.bin")).toBeNull());
  const getIndex = host.calls.findIndex(
    (c) => c.method === "git.conflict.get" && c.params.path === "image.bin",
  );
  const resolveIndex = host.calls.findIndex((c) => c.method === "git.conflict.resolve");
  expect(getIndex).toBeGreaterThanOrEqual(0);
  expect(resolveIndex).toBeGreaterThan(getIndex);
  expect(calls("git.conflict.resolve")[0]).toMatchObject({
    projectId: "p1",
    path: "image.bin",
    side: { pick: "ours", expectedVersion: "fixture-version" },
  });

  // [병합…] → 머지 문서.
  await click(conflictRow("notes.txt")!.querySelector("[data-git-merge-open]")!);
  await vi.waitFor(() =>
    expect(document.querySelector("[data-git-merge='notes.txt']")).not.toBeNull(),
  );
  await vi.waitFor(() => expect(pane("result")).toBeDefined());
  await settle();
  expect(pane("result").state.doc.toString()).toBe(FIXTURE.detail.merged);
  expect(pane("ours").state.doc.toString()).toBe(
    FIXTURE.detail.ours!.content!.kind === "text" ? FIXTURE.detail.ours!.content!.text : "",
  );
  expect(unresolvedText()).toBe("남은 충돌 2개");

  // 첫 충돌은 ≫.
  await click(regionButton("ours", 0, "apply")!);
  expect(pane("result").state.doc.toString()).toContain("alpha\none ours\ntwo\n");
  expect(unresolvedText()).toBe("남은 충돌 1개");
  // 고른 쪽의 단추는 거둔다 — 그쪽(≪)은 아직 있다.
  expect(regionButton("ours", 0, "apply")).toBeNull();
  expect(regionButton("theirs", 0, "apply")).not.toBeNull();

  // 둘째 충돌(구간 2)은 직접 친다.
  await act(async () => {
    const at = rangeOf(2).from;
    pane("result").dispatch({
      changes: { from: at, insert: FIXTURE.typed },
      userEvent: "input.type",
    });
  });
  await settle();
  expect(unresolvedText()).toBe("모든 충돌을 골랐다");

  // 저장 = 그 결과를 get 의 version 으로.
  await click(document.querySelector("[data-git-merge-save]")!);
  await vi.waitFor(() => expect(calls("git.conflict.resolve")).toHaveLength(2));
  expect(calls("git.conflict.resolve")[1]).toEqual({
    projectId: "p1",
    worktree: null,
    path: "notes.txt",
    content: { text: FIXTURE.expected, bom: false, expectedVersion: "fixture-version" },
  });
  // 풀었으면 탭을 닫는다.
  await vi.waitFor(() => expect(document.querySelector("[data-git-merge='notes.txt']")).toBeNull());

  // [계속] → git.operation.continue, 끝나면 알림.
  await click(bannerButton("continue")!);
  await vi.waitFor(() => expect(calls("git.operation.continue")).toHaveLength(1));
  await vi.waitFor(() => expect(toasts()).toContain("머지을(를) 끝냈다"));
  expect(calls("git.operation.abort")).toHaveLength(0);
});

it("aborts a rebase from the banner only after the confirmation", async () => {
  host.operation = FIXTURE.rebase.operation;
  host.conflicts = { workTree: "/repo", files: [] };
  await mount();
  await vi.waitFor(() => expect(banner()?.dataset.operation).toBe("rebase"));
  expect(banner()!.textContent).toContain("리베이스 진행 중 · main → 4489558 · 1/1 단계");
  expect(bannerButton("skip")).not.toBeNull();

  // 취소하면 부르지 않는다.
  await click(bannerButton("abort")!);
  await vi.waitFor(() => expect(confirmDialog()).not.toBeNull());
  expect(confirmDialog()!.textContent).toContain("리베이스을(를) 중단할까?");
  expect(calls("git.operation.abort")).toHaveLength(0);
  const [cancel] = confirmDialog()!.querySelectorAll("button");
  await click(cancel!);
  expect(confirmDialog()).toBeNull();
  expect(calls("git.operation.abort")).toHaveLength(0);

  await click(bannerButton("abort")!);
  await vi.waitFor(() => expect(confirmDialog()).not.toBeNull());
  const buttons = confirmDialog()!.querySelectorAll("button");
  await click(buttons[buttons.length - 1]!);
  await vi.waitFor(() =>
    expect(calls("git.operation.abort")).toEqual([{ projectId: "p1", worktree: null }]),
  );
  await vi.waitFor(() => expect(toasts()).toContain("리베이스을(를) 중단했다"));
  // Host 가 HEAD 를 돌렸다 → 진행 중 작업이 없다고 다시 읽으면 배너가 사라진다.
  host.operation = null;
  await act(async () => host.emit("git.refsChanged", { worktree: null }));
  await vi.waitFor(() => expect(banner()).toBeNull());
});

it("edits the result with Korean IME composition and saves what was composed", async () => {
  await mount();
  await openMerge();
  const view = pane("result");
  // 구간 0(빈 충돌) 자리 = 「two」 줄 앞. 그 줄 앞에 조합해 넣는다.
  const at = rangeOf(0).from;
  // 입력기가 고치는 것은 지금 그 줄의 글자 노드다 — 장식이 바뀌어 줄을 다시 그리면 노드가 바뀌므로 매번 다시 찾는다.
  const node = () => {
    const found = view.domAtPos(view.state.doc.lineAt(at).from).node;
    return found.nodeType === Node.TEXT_NODE ? (found as Text) : (found.firstChild as Text);
  };
  expect(node().data).toBe("two");
  const step = async (type: string, data: string, text?: string) => {
    view.contentDOM.dispatchEvent(new CompositionEvent(type, { data }));
    if (text !== undefined) node().data = text;
    await new Promise((r) => setTimeout(r, 20));
  };
  await act(async () => {
    await step("compositionstart", "", "ㅎtwo");
    await step("compositionupdate", "하", "하two");
    await step("compositionupdate", "한", "한two");
    await step("compositionupdate", "한ㄱ", "한ㄱtwo");
    await step("compositionupdate", "한글", "한글two");
    await step("compositionend", "한글");
    await new Promise((r) => setTimeout(r, 30));
  });
  await settle();
  expect(view.state.doc.toString()).toContain("alpha\n한글two\nthree\n");
  // 조합으로 친 글은 그 충돌을 고친 것이다.
  expect(unresolvedText()).toBe("남은 충돌 1개");

  host.calls = [];
  await click(document.querySelector("[data-git-merge-save]")!);
  // 고르지 않은 충돌이 남아 먼저 묻는다.
  await vi.waitFor(() => expect(confirmDialog()).not.toBeNull());
  const buttons = confirmDialog()!.querySelectorAll("button");
  await click(buttons[buttons.length - 1]!);
  await vi.waitFor(() => expect(calls("git.conflict.resolve")).toHaveLength(1));
  const sent = calls("git.conflict.resolve")[0]!.content as { text: string };
  expect(sent.text).toBe(view.state.doc.toString());
  expect(sent.text).toContain("한글two\n");
});

it("re-reads on CONFLICT, keeps the result and saves with the new version", async () => {
  await mount();
  await openMerge();
  await click(regionButton("ours", 0, "apply")!);
  await click(regionButton("theirs", 2, "apply")!);
  const text = pane("result").state.doc.toString();
  host.refuseResolve = { code: -32004, message: "changed" };
  host.details.set("notes.txt", { ...structuredClone(FIXTURE.detail), version: "v2" });
  await click(document.querySelector("[data-git-merge-save]")!);
  await vi.waitFor(() => expect(toasts()).toContain("파일이 그사이 바뀌어 저장하지 않았다"));
  expect(pane("result").state.doc.toString()).toBe(text);
  await click(document.querySelector("[data-git-merge-save]")!);
  await vi.waitFor(() => expect(calls("git.conflict.resolve")).toHaveLength(2));
  expect(calls("git.conflict.resolve")[1]!.content).toEqual({
    text,
    bom: false,
    expectedVersion: "v2",
  });
});

it("✕ puts a one-sided change back to base and «apply all non-conflicting» redoes it; base compare toggles", async () => {
  await mount();
  await openMerge();
  const applyAll = () => document.querySelector<HTMLButtonElement>("[data-git-merge-apply-all]")!;
  expect(applyAll().disabled).toBe(true);
  await click(regionButton("theirs", 1, "ignore")!);
  expect(pane("result").state.doc.toString()).toContain("mid1\nmid2\nmid3\n");
  expect(applyAll().disabled).toBe(false);
  await click(applyAll());
  expect(pane("result").state.doc.toString()).toBe(FIXTURE.detail.merged);

  const base = document.querySelector<HTMLElement>("[data-git-merge-base]")!;
  await click(base);
  expect(base.getAttribute("aria-pressed")).toBe("true");
  // 통합 보기: ours 칸에 base 와의 차이 표시가 붙는다.
  await vi.waitFor(() =>
    expect(document.querySelector("[data-git-merge-pane=ours] .cm-changedLine")).not.toBeNull(),
  );
  await click(base);
  expect(document.querySelector("[data-git-merge-pane=ours] .cm-changedLine")).toBeNull();
});

it("offers only a side pick for a binary conflict", async () => {
  await mount();
  await act(async () => {
    windowRegistry()!.run("git.openMerge", { path: "image.bin" });
  });
  await vi.waitFor(() =>
    expect(document.querySelector("[data-git-merge-pick-only=binary]")).not.toBeNull(),
  );
  await click(document.querySelector("[data-git-merge-pick-only] [data-git-pick=theirs]")!);
  await vi.waitFor(() =>
    expect(calls("git.conflict.resolve")).toEqual([
      expect.objectContaining({
        path: "image.bin",
        side: { pick: "theirs", expectedVersion: "fixture-version" },
      }),
    ]),
  );
});

it("shows a clean conflict as three panes with nothing to pick and still resolves it on save", async () => {
  const ours = FIXTURE.detail.ours!.content as { text: string };
  host.details.set("notes.txt", {
    ...structuredClone(FIXTURE.detail),
    merged: ours.text,
    regions: [],
    clean: true,
  });
  await mount();
  await openMerge();
  expect(unresolvedText()).toContain("고를 충돌 구간이 없다");
  expect(document.querySelector("[data-merge-action]")).toBeNull();
  await click(document.querySelector("[data-git-merge-save]")!);
  // 고를 것이 없으니 묻지 않고 푼다(index 는 아직 충돌이다).
  expect(confirmDialog()).toBeNull();
  await vi.waitFor(() =>
    expect(calls("git.conflict.resolve")).toEqual([
      expect.objectContaining({
        path: "notes.txt",
        content: { text: ours.text, bom: false, expectedVersion: "fixture-version" },
      }),
    ]),
  );
});

it("turns Merge… off for a file over 5 MiB and says why", async () => {
  host.conflicts = {
    ...host.conflicts,
    files: host.conflicts.files.map((f) => (f.path === "notes.txt" ? { ...f, large: true } : f)),
  };
  await mount();
  await act(async () => {
    windowRegistry()!.run("git.resolveConflicts");
  });
  await vi.waitFor(() => expect(conflictRow("notes.txt")).not.toBeNull());
  const merge =
    conflictRow("notes.txt")!.querySelector<HTMLButtonElement>("[data-git-merge-open]")!;
  expect(merge.disabled).toBe(true);
  expect(merge.title).toContain("5 MiB");
  // 한쪽 고르기는 된다.
  expect(
    conflictRow("notes.txt")!.querySelector<HTMLButtonElement>("[data-git-pick=theirs]")!.disabled,
  ).toBe(false);
});
