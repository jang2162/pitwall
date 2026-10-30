// @vitest-environment happy-dom
// 커밋 툴 윈도우(13-m2-plan M2-16 완료 기준)를 프로젝트 창(App kind="project")과 가짜 Host 로 본다. 시나리오 2 의 답은 진짜 pw-git ·
// git 의 출력이다(fixtures/stage-commit.json — crates/pw-git `commit::tests::window_fixture` 가 만들고, 그 JSON 이 지금 pw-git · git 의
// 출력과 같은지 본다): 체크를 눌러 창이 보낸 `git.stageLines` 요청이 그 JSON 의 요청과 같으면, 그 요청을 진짜 git 으로 돌린 뒤의
// `git show --stat` 이 JSON 의 `stat` 이다.
//
// 위험 작업 확인(M2-20 의 부품 — 변경 되돌리기 · 묶음 되돌리기 · stash 버리기)과 amend 의 [되돌리기] 는 fixtures/discard.json 이 답이다
// (crates/pw-git `commit::tests::discard_fixture` 가 진짜 pw-git · git 으로 돌려 만들고 같은지 본다): 확인 목록이 그 git 이 실제로 바꾼
// 것과 같고, 취소하면 아무것도 부르지 않고, 확인하면 보낸 요청이 그 git 으로 돌린 요청과 같다.
//
// 액션은 키 · 다른 행이 부르는 길 그대로 부른다(⌘↩ 키, `registry.run`).

import { ActionRegistry } from "@pitwall/keymap";
import type {
  ConfigView,
  FsRead,
  GitBlob,
  GitDiff,
  GitStatusEntry,
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
import { diffPreferences } from "../documents/changes-view";
import { resetWindowGitStatus } from "../git-status";
import { commitPanel, resetCommitPanel } from "./commit-state";
import discardFixture from "./fixtures/discard.json";
import fixture from "./fixtures/stage-commit.json";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

interface Step {
  diff: GitDiff;
  index: GitBlob;
  workTree: string;
  request?: { path: string; hunks: unknown[] };
}

const FIXTURE = fixture as unknown as {
  path: string;
  message: string;
  steps: Step[];
  staged: Step;
  stat: string;
  patch: string;
  committedMessage: string;
};

const host = vi.hoisted(() => ({
  calls: [] as { method: string; params: Record<string, unknown> }[],
  /** 지금 단계(`FIXTURE.steps` 의 인덱스, 끝이면 `staged`). */
  step: 0,
  entries: [] as unknown[],
  template: null as string | null,
  headMessage: "previous subject\n\nprevious body\n",
  /** 이 메시지로 커밋을 거절한다(훅 실패 — git 오류 2004 의 stderr). */
  rejectWith: undefined as string | undefined,
  /** 설정의 코드 글꼴(`[editor]`). */
  editor: undefined as Record<string, unknown> | undefined,
  /** 주석 글자(`core.commentChar`). */
  commentChar: "#",
  /** 있으면 `git.commit` 의 답을 이것이 풀릴 때까지 미룬다. */
  holdCommit: undefined as Promise<void> | undefined,
  /** 진행 중 작업(`git.operation`). */
  operation: null as unknown,
  emit: (_topic: string, _payload: unknown) => {},
  fixture: undefined as unknown as Pick<typeof FIXTURE, "steps" | "staged">,
  /** `git.stash.list` 의 답. */
  stashes: [] as unknown[],
  /** 있으면 `git.commit` 의 `moved`. */
  moved: undefined as unknown,
  /** 작업 트리에서 폴더인 경로(서브모듈 — `fs.stat` 의 `kind`). */
  dirs: new Set<string>(),
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
        roots: ["/repo"],
        favorite: false,
        tags: [],
        configMode: "repository",
        slug: "demo",
        createdAt: 1,
        openedAt: null,
      };
      const state = () => host.fixture.steps[host.step] ?? host.fixture.staged;
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const reply = (result: unknown) => send({ id: m.id, result });
        const params = (m.params ?? {}) as Record<string, unknown>;
        host.calls.push({ method: m.method, params });
        switch (m.method) {
          case "i18n.get":
            return reply(i18nState("ko"));
          case "config.get":
            return reply({
              snapshot: {
                files: host.editor
                  ? [{ id: "settings", value: { editor: host.editor } } as never]
                  : [],
                notices: [],
              },
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
          case "fs.stat": {
            const rel = (params.path as string).replace(/^\/repo\//, "");
            return reply({
              path: params.path,
              kind: host.dirs.has(rel) ? "dir" : "file",
              target: null,
              size: 1,
              mtimeMs: 1,
              readonly: false,
              sizeClass: host.dirs.has(rel) ? null : "editable",
            });
          }
          case "fs.list":
            return reply({ path: params.path, entries: [] });
          case "fs.read":
            return reply({
              path: params.path as string,
              size: state().workTree.length,
              mtimeMs: 1,
              offset: 0,
              eof: true,
              version: "v1",
              content: { kind: "text", text: state().workTree },
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
          case "git.refs":
            return reply({
              workTree: "/repo",
              head: null,
              branches: [],
              remoteBranches: [],
              tags: [],
              unsupported: null,
              broken: [],
            });
          case "git.operation":
            return reply({ operation: host.operation });
          case "git.watch": {
            const streamId = next++;
            send({ id: m.id, result: { streamId } });
            return send({
              method: "stream.chunk",
              params: { streamId, data: { workTree: "/repo" } },
            });
          }
          case "git.status": {
            const paths = params.paths as string[] | null;
            const entries = (host.entries as GitStatusEntry[]).filter(
              (e) => !paths || paths.includes(e.path),
            );
            return reply({ workTree: "/repo", entries, unsupported: null });
          }
          case "git.repo":
            return reply({ repo: { workTree: "/repo" } });
          case "git.diff":
            return reply(state().diff);
          case "git.blob":
            return reply(state().index);
          case "git.show":
            return reply({ message: host.headMessage });
          case "git.commit.template":
            return reply({ template: host.template, commentChar: host.commentChar });
          case "git.stash.list":
            return reply({ workTree: "/repo", stashes: host.stashes });
          case "git.stash.drop":
          case "git.discardLines":
            return reply({ workTree: "/repo" });
          case "git.reset":
            return reply({
              moved: {
                refName: params.expectedRef,
                previous: params.expected,
                current: params.commit,
              },
              workTree: "/repo",
            });
          case "git.stageLines":
            host.step++;
            return reply({ workTree: "/repo" });
          case "git.stage":
          case "git.unstage":
            return reply({ workTree: "/repo" });
          case "git.discard":
            return reply({ workTree: "/repo", untracked: [] });
          case "git.commit":
            if (host.holdCommit) {
              const held = host.holdCommit;
              host.holdCommit = undefined;
              void held.then(() =>
                reply({
                  commit: "c0ffee0000000000000000000000000000000000",
                  moved: {
                    refName: "refs/heads/main",
                    previous: "aaaaaaa000000000000000000000000000000000",
                    current: "c0ffee0000000000000000000000000000000000",
                  },
                  workTree: "/repo",
                  output: "",
                }),
              );
              return;
            }
            if (host.rejectWith !== undefined)
              return send({
                id: m.id,
                error: {
                  code: 2004,
                  message: "git commit failed",
                  data: { stderr: host.rejectWith },
                },
              });
            return reply({
              commit: "c0ffee0000000000000000000000000000000000",
              moved: host.moved ?? {
                refName: "refs/heads/main",
                previous: "aaaaaaa000000000000000000000000000000000",
                current: "c0ffee0000000000000000000000000000000000",
              },
              workTree: "/repo",
              output: "pre-commit: ok\n",
            });
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
            });
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

const MODIFIED: GitStatusEntry = {
  path: "src/app.rs",
  origPath: null,
  staged: null,
  unstaged: "modified",
  conflict: null,
};

let root: Root | undefined;
let theme: ThemeStore | undefined;
let registries = new Set<ActionRegistry>();
const windowRegistry = () => [...registries].at(-1)!;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  fakeTreeLayout(600);
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.calls = [];
  host.step = 0;
  host.entries = [MODIFIED];
  host.template = null;
  host.rejectWith = undefined;
  host.editor = undefined;
  host.operation = null;
  host.commentChar = "#";
  host.holdCommit = undefined;
  host.fixture = FIXTURE;
  host.stashes = [];
  host.moved = undefined;
  host.dirs = new Set();
  activeWorktreeStore.setState({ top: undefined });
  diffPreferences.setState({ mode: "split" });
  resetWindowFileTree();
  resetWindowGitStatus();
  resetCommitPanel();
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
  await vi.waitFor(() => expect(windowRegistry().get("git.commit")).toBeDefined());
}

async function click(el: Element, init: MouseEventInit = {}) {
  await act(async () => {
    el.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, cancelable: true, ...init }));
    el.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true, ...init }));
    el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, ...init }));
  });
  await settle();
}

async function key(el: Element, init: KeyboardEventInit) {
  await act(async () => {
    el.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }));
  });
  await settle();
}

const row = (key: string) => document.querySelector<HTMLElement>(`[data-commit-item="${key}"]`);
const messageBox = () => document.querySelector<HTMLTextAreaElement>("[data-commit-message]");
const calls = (method: string) => host.calls.filter((c) => c.method === method);

/** ⌘K 로 커밋 툴 윈도우를 연다(메시지 칸에 포커스가 든다). `first` 행이 그려질 때까지 기다린다. */
async function openCommitWindow(first = "changes:src/app.rs") {
  await mount();
  await act(async () => {
    windowRegistry().run("git.commit");
  });
  await vi.waitFor(() => expect(row(first)).not.toBeNull());
  await vi.waitFor(() => expect(document.activeElement).toBe(messageBox()));
}

async function type(text: string) {
  const box = messageBox()!;
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!;
    setter.call(box, text);
    box.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();
}

/** 미리보기 diff 의 거터 칸. `side` 는 나란히 보기의 옛 쪽(a) · 새 쪽(b). */
function stageBox(side: "a" | "b", line: number, what: "group" | "line") {
  const pane = document.querySelector(`.cm-merge-${side}`);
  return pane?.querySelector<HTMLInputElement>(
    `.pw-stage[data-stage-at="${line}"] [data-stage-${what}]`,
  );
}

/** 단계의 diff 에서 글자가 같은 바뀐 줄의 줄 번호(옛 쪽 · 새 쪽). */
function lineOf(step: Step, text: string) {
  for (const h of step.diff.files[0]!.hunks)
    for (const l of h.lines)
      if (l.kind !== "context" && l.text === text)
        return {
          side: l.kind === "added" ? ("b" as const) : ("a" as const),
          n: (l.newLine ?? l.oldLine)!,
        };
  throw new Error(`no line ${text}`);
}

it("stages one hunk and two lines of another from the diff preview, then commits with ⌘↩ (scenario 2)", async () => {
  await openCommitWindow();
  await click(row("changes:src/app.rs")!);
  // 고른 파일의 diff 미리보기 탭(작업 트리 diff)에 묶음 · 줄 체크가 있다.
  await vi.waitFor(() => expect(stageBox("b", 2, "group")).toBeTruthy());

  // 1) 첫 hunk 를 묶음 체크로.
  await click(stageBox("b", 2, "group")!);
  await vi.waitFor(() => expect(calls("git.stageLines")).toHaveLength(1));
  // 2 · 3) 둘째 hunk 의 줄 둘을 줄 체크로 — 단계마다 다시 읽은 diff 에서 고른다.
  for (const [i, text] of [
    [1, "    b1 new"],
    [2, "    b1 old"],
  ] as const) {
    const at = lineOf(FIXTURE.steps[i]!, text);
    await vi.waitFor(() => expect(stageBox(at.side, at.n, "line")).toBeTruthy());
    await click(stageBox(at.side, at.n, "line")!);
    await vi.waitFor(() => expect(calls("git.stageLines")).toHaveLength(i + 1));
  }
  // 창이 보낸 요청이 진짜 git 으로 돌린 요청과 같다.
  expect(
    calls("git.stageLines").map(({ params }) => ({ path: params.path, hunks: params.hunks })),
  ).toEqual(FIXTURE.steps.map((s) => s.request));
  expect(calls("git.stageLines").every((c) => c.params.ignoreWhitespace === false)).toBe(true);

  host.entries = [{ ...MODIFIED, staged: "modified" }];
  host.emit("git.statusChanged", { worktree: null, paths: null });
  await vi.waitFor(() => expect(row("staged:src/app.rs")).not.toBeNull());
  expect(row("changes:src/app.rs")!.textContent).toContain("부분");

  await act(async () => messageBox()!.focus());
  await type(FIXTURE.message);
  await key(messageBox()!, { key: "Enter", code: "Enter", metaKey: true });
  await vi.waitFor(() => expect(calls("git.commit")).toHaveLength(1));
  expect(calls("git.commit")[0]!.params).toMatchObject({
    message: FIXTURE.message,
    amend: false,
    signoff: false,
  });
  // 그 요청들로 진짜 git 이 만든 커밋: 첫 hunk 와 둘째 hunk 의 두 줄만.
  expect(FIXTURE.stat).toContain("src/app.rs | 4 ++--");
  expect(FIXTURE.committedMessage).toBe(FIXTURE.message);
  // 커밋 뒤 칸이 비고, 훅 출력이 남고, 메시지가 기록에 든다.
  await vi.waitFor(() => expect(messageBox()!.value).toBe(""));
  expect(document.querySelector('[data-commit-output="ok"]')?.textContent).toContain(
    "pre-commit: ok",
  );
  await click(document.querySelector("[data-commit-history]")!);
  expect(document.querySelector("[data-commit-history-list]")?.textContent).toContain("Change app");
});

it("shows a failing pre-commit hook's output and keeps the message", async () => {
  host.entries = [{ ...MODIFIED, staged: "modified" }];
  host.rejectWith = "lint: src/app.rs:3 unused\n";
  await openCommitWindow();
  await type("fix: things\n");
  await click(document.querySelector("[data-commit-run]")!);
  await vi.waitFor(() =>
    expect(document.querySelector('[data-commit-output="failed"]')?.textContent).toContain(
      "lint: src/app.rs:3 unused",
    ),
  );
  expect(messageBox()!.value).toBe("fix: things\n");
  expect(commitPanel.getState().busy).toBe(false);
});

it("moves a file to the staged group within 500 ms of a terminal git add", async () => {
  await openCommitWindow();
  expect(row("staged:src/app.rs")).toBeNull();
  host.entries = [{ ...MODIFIED, staged: "modified", unstaged: null }];
  const started = performance.now();
  await act(async () => host.emit("git.statusChanged", { worktree: null, paths: ["src/app.rs"] }));
  await vi.waitFor(() => expect(row("staged:src/app.rs")).not.toBeNull(), { interval: 5 });
  const took = performance.now() - started;
  expect(row("changes:src/app.rs")).toBeNull();
  expect(took).toBeLessThan(500);
});

it("prefills commit.template and drops its untouched comment lines on commit", async () => {
  host.entries = [{ ...MODIFIED, staged: "modified" }];
  host.template = "Subject\n\n# Explain why.\n";
  await openCommitWindow();
  await vi.waitFor(() => expect(messageBox()!.value).toBe(host.template));
  await type("feat: add x\n\n# Explain why.\n#123 is the issue\n");
  await act(async () => {
    windowRegistry().run("git.commit.run");
  });
  await vi.waitFor(() => expect(calls("git.commit")).toHaveLength(1));
  expect(calls("git.commit")[0]!.params.message).toBe("feat: add x\n\n#123 is the issue\n");
  // 커밋 뒤에 템플릿을 다시 채운다.
  await vi.waitFor(() => expect(messageBox()!.value).toBe(host.template));
});

it("fills the HEAD message for amend and restores the box when amend is turned off", async () => {
  await openCommitWindow();
  await type("draft");
  await click(document.querySelector("[data-commit-amend]")!);
  // 칸이 비어 있지 않으면 덮지 않는다.
  expect(messageBox()!.value).toBe("draft");
  await click(document.querySelector("[data-commit-amend]")!);
  await type("");
  await click(document.querySelector("[data-commit-amend]")!);
  await vi.waitFor(() => expect(messageBox()!.value).toBe(host.headMessage));
  await click(document.querySelector("[data-commit-signoff]")!);
  await act(async () => {
    windowRegistry().run("git.commit.run");
  });
  await vi.waitFor(() => expect(calls("git.commit")).toHaveLength(1));
  expect(calls("git.commit")[0]!.params).toMatchObject({ amend: true, signoff: true });
  await vi.waitFor(() => expect(commitPanel.getState().amend).toBe(false));
});

it("does not roll back when the confirmation is cancelled", async () => {
  await openCommitWindow();
  await click(row("changes:src/app.rs")!);
  await act(async () => {
    windowRegistry().run("git.rollback");
  });
  const cancel = await vi.waitFor(() => {
    const b = [...document.querySelectorAll("button")].find((x) => x.textContent === "취소");
    expect(b).toBeDefined();
    return b!;
  });
  await click(cancel);
  await settle(50);
  expect(calls("git.discard")).toHaveLength(0);
  expect(calls("git.unstage")).toHaveLength(0);
});

it("rolls back the selected file with ⌘⌥Z only after the confirmation", async () => {
  await openCommitWindow();
  await click(row("changes:src/app.rs")!);
  await act(async () =>
    row("changes:src/app.rs")!.closest<HTMLElement>("[data-commit-list]")!.focus(),
  );
  await key(document.activeElement!, { key: "z", code: "KeyZ", metaKey: true, altKey: true });
  // 다이얼로그 안의 단추다 — 앞 시험의 토스트(sonner 는 시험 사이에 남는다)에도 「되돌리기」 가 있다.
  const confirm = dialogButton(await confirmDialog(), "되돌리기");
  expect(calls("git.discard")).toHaveLength(0);
  await click(confirm);
  await vi.waitFor(() => expect(calls("git.discard")).toHaveLength(1));
  expect(calls("git.discard")[0]!.params).toMatchObject({ paths: ["src/app.rs"], confirmed: true });
});

it("stages a file with its checkbox and unstages it from the staged group", async () => {
  host.entries = [{ ...MODIFIED, staged: "added", unstaged: null, path: "src/new.rs" }, MODIFIED];
  await openCommitWindow();
  await click(row("changes:src/app.rs")!.querySelector("[data-commit-check]")!);
  await vi.waitFor(() => expect(calls("git.stage")).toHaveLength(1));
  expect(calls("git.stage")[0]!.params.paths).toEqual(["src/app.rs"]);
  await click(row("staged:src/new.rs")!.querySelector("[data-commit-check]")!);
  await vi.waitFor(() => expect(calls("git.unstage")).toHaveLength(1));
  expect(calls("git.unstage")[0]!.params.paths).toEqual(["src/new.rs"]);
});

it("commit and push opens git.push.open when that action is in the window", async () => {
  host.entries = [{ ...MODIFIED, staged: "modified" }];
  await openCommitWindow();
  const pushed = vi.fn();
  windowRegistry().register({ id: "git.push.open", label: "push", run: pushed });
  await type("feat: push me\n");
  await key(messageBox()!, { key: "k", code: "KeyK", metaKey: true, altKey: true });
  await vi.waitFor(() => expect(pushed).toHaveBeenCalledTimes(1));
  expect(calls("git.commit")).toHaveLength(1);
});

it("uses the editor font setting in the diff preview", async () => {
  host.editor = { font_family: "Iosevka Test", font_size: 17 };
  await openCommitWindow();
  await click(row("changes:src/app.rs")!);
  const scroller = await vi.waitFor(() => {
    const s = document.querySelector<HTMLElement>(".cm-merge-b .cm-scroller");
    expect(s).toBeTruthy();
    return s!;
  });
  expect(getComputedStyle(scroller).fontFamily).toContain("Iosevka Test");
  expect(getComputedStyle(scroller.closest(".cm-editor")!).fontSize).toBe("17px");
});

it("shows the operation-in-progress banner (M2-19) above the list while a merge is stopped", async () => {
  host.operation = {
    kind: "merge",
    heads: ["b".repeat(40)],
    branch: null,
    onto: null,
    step: null,
    remaining: null,
    actions: ["continue", "abort"],
  };
  host.entries = [{ ...MODIFIED, unstaged: null, conflict: "bothModified" }];
  await mount();
  await act(async () => {
    windowRegistry().run("git.commit");
  });
  const banner = await vi.waitFor(() => {
    const b = document.querySelector<HTMLElement>(
      '[data-tool-window="commit"] [data-testid="git-operation-banner"]',
    );
    expect(b).not.toBeNull();
    return b!;
  });
  expect(banner.dataset.placement).toBe("commit");
  expect(banner.dataset.operation).toBe("merge");
  // 배너는 목록보다 위에 있다.
  const list = document.querySelector("[data-commit-list]")!;
  expect(banner.compareDocumentPosition(list) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(row("conflicts:src/app.rs")).not.toBeNull();
});

it("rolls back a staged rename with the old path too (review F2)", async () => {
  host.entries = [
    {
      path: "src/new.rs",
      origPath: "src/old.rs",
      staged: "renamed",
      unstaged: null,
      conflict: null,
    },
    MODIFIED,
  ];
  await openCommitWindow();
  await click(row("staged:src/new.rs")!);
  await act(async () => {
    windowRegistry().run("git.rollback");
  });
  await click(dialogButton(await confirmDialog(), "되돌리기"));
  await vi.waitFor(() => expect(calls("git.discard")).toHaveLength(1));
  expect(calls("git.unstage")[0]!.params.paths).toEqual(["src/new.rs", "src/old.rs"]);
  expect(calls("git.discard")[0]!.params).toMatchObject({
    paths: ["src/new.rs", "src/old.rs"],
    confirmed: true,
  });
});

it("does not commit an unedited template and uses core.commentChar (review F5)", async () => {
  host.entries = [{ ...MODIFIED, staged: "modified" }];
  host.template = "Subject\n\n; Explain why.\n";
  host.commentChar = ";";
  await openCommitWindow();
  await vi.waitFor(() => expect(messageBox()!.value).toBe(host.template));
  await act(async () => {
    windowRegistry().run("git.commit.run");
  });
  await settle(50);
  expect(calls("git.commit")).toHaveLength(0);
  await type("feat: real subject\n\n; Explain why.\n");
  await act(async () => {
    windowRegistry().run("git.commit.run");
  });
  await vi.waitFor(() => expect(calls("git.commit")).toHaveLength(1));
  expect(calls("git.commit")[0]!.params.message).toBe("feat: real subject\n");
});

it("locks the message box while committing (review F6)", async () => {
  host.entries = [{ ...MODIFIED, staged: "modified" }];
  let release!: () => void;
  host.holdCommit = new Promise((r) => (release = r));
  await openCommitWindow();
  await type("feat: slow hook\n");
  await click(document.querySelector("[data-commit-run]")!);
  await vi.waitFor(() => expect(calls("git.commit")).toHaveLength(1));
  expect(messageBox()!.readOnly).toBe(true);
  await act(async () => release());
  await vi.waitFor(() => expect(messageBox()!.readOnly).toBe(false));
  expect(messageBox()!.value).toBe("");
});

interface Rollback {
  entries: GitStatusEntry[];
  unstage: string[];
  discard: string[];
  lost: string[];
  lostIndex: string[];
  kept: string[];
  untouched: string[];
}

const DISCARD = discardFixture as unknown as {
  rollback: Rollback;
  rollbackChanges: Rollback;
  rollbackSubmodule: Rollback;
  hunk: Step & { path: string; request: { path: string; hunks: unknown[] }; after: string };
  stash: {
    stashes: { index: number; commit: string; message: string }[];
    request: { index: number; commit: string };
    after: unknown[];
  };
  amend: { moved: { refName: string; previous: string; current: string }; request: object };
};

/** 떠 있는 위험 작업 확인(M2-20 의 부품). */
const confirmDialog = () =>
  vi.waitFor(() => {
    const d = document.querySelector<HTMLElement>('[data-testid="git-confirm-dialog"]');
    expect(d).not.toBeNull();
    return d!;
  });

const dialogButton = (dialog: HTMLElement, label: string) =>
  [...dialog.querySelectorAll("button")].find((b) => b.textContent === label)!;

/** 위험 표시가 있고 처음 포커스가 취소다(M2-20 의 다른 확인과 같다). */
function expectDanger(dialog: HTMLElement) {
  expect(dialog.dataset.danger).toBe("true");
  expect(dialog.querySelector('[aria-label="Pitwall 에서 되돌릴 수 없다"]')).not.toBeNull();
  expect(document.activeElement).toBe(dialogButton(dialog, "취소"));
}

/** 제목에 `title` 이 든 토스트(sonner — 앞 시험의 토스트가 남아 있다)의 단추. */
const toastButton = (title: string, label: string) =>
  vi.waitFor(() => {
    const toast = [...document.querySelectorAll<HTMLElement>("[data-sonner-toast]")].find((t) =>
      t.textContent?.includes(title),
    );
    const b = [...(toast?.querySelectorAll("button") ?? [])].find((e) => e.textContent === label);
    if (!b) throw new Error(`no toast button ${label} for ${title}`);
    return b;
  });

/** 확인의 잃는 목록 줄. */
const lostLines = (dialog: HTMLElement) =>
  [...dialog.querySelectorAll('[data-testid="git-lost-list"] li')].map((li) => li.textContent!);

/** 확인 본문에서 `head` 가 든 줄 아래(빈 줄까지)의 줄. 없으면 빈 목록. */
function bodySection(dialog: HTMLElement, head: string): string[] {
  const body = dialog.querySelector(".whitespace-pre-line")!.textContent!.split("\n");
  const at = body.findIndex((l) => l.includes(head));
  if (at < 0) return [];
  const rest = body.slice(at + 1);
  const end = rest.indexOf("");
  return end < 0 ? rest : rest.slice(0, end);
}

/** 잃는 목록이 git 이 실제로 잃은 경로와 같다: 디스크가 바뀐 것(이름 바뀜 「옛 → 새」 은 돌아오는 옛 경로) · 스테이징한 내용이 사라진 것. */
function expectLostLike(dialog: HTMLElement, fx: Rollback) {
  expect(new Set(lostLines(dialog).map((l) => l.split(" → ")[0]))).toEqual(
    new Set([...fx.lost, ...fx.lostIndex]),
  );
  // 남는 것: 「지우지 않고」 아래 줄이 git 이 디스크에 그대로 둔 파일이고, 스테이징한 내용을 잃는 것은 그렇게 적혀 있다.
  const kept = bodySection(dialog, "지우지 않고");
  expect(kept.map((l) => l.split(" — ")[0])).toEqual(fx.kept);
  expect(
    kept.filter((l) => l.includes("스테이징한 내용은 잃는다")).map((l) => l.split(" — ")[0]),
  ).toEqual(fx.kept.filter((p) => fx.lostIndex.includes(p)));
  expect(bodySection(dialog, "건드리지 않는다")).toEqual(fx.untouched);
}

/** 변경 그룹(`stagedToo` 면 스테이징됨 그룹도)의 파일을 모두 고르고 ⌘⌥Z 의 확인을 띄운다. */
async function rollbackEverything({ entries }: Rollback, stagedToo = true) {
  const staged = stagedToo
    ? entries.filter((e) => e.staged && e.unstaged !== "untracked").map((e) => e.path)
    : [];
  const changes = entries
    .filter((e) => e.unstaged && e.unstaged !== "untracked")
    .map((e) => e.path);
  await click(row(stagedToo ? `staged:${staged[0]}` : `changes:${changes[0]}`)!);
  await click(row(`changes:${changes.at(-1)}`)!, { shiftKey: true });
  expect(commitPanel.getState().selected).toHaveLength(staged.length + changes.length);
  await act(async () => {
    windowRegistry().run("git.rollback");
  });
  return confirmDialog();
}

it("lists exactly what git changes on disk before rolling back, and nothing when cancelled", async () => {
  host.entries = DISCARD.rollback.entries;
  await openCommitWindow("staged:added.txt");
  const dialog = await rollbackEverything(DISCARD.rollback);
  expectDanger(dialog);
  expect(lostLines(dialog)).toEqual([
    "added-gone.txt",
    "am.txt",
    "gone.txt",
    "mod.txt",
    "old.txt → new.txt",
    "staged.txt",
    "old.txt",
  ]);
  expectLostLike(dialog, DISCARD.rollback);
  await click(dialogButton(dialog, "취소"));
  await settle(50);
  expect(calls("git.unstage")).toHaveLength(0);
  expect(calls("git.discard")).toHaveLength(0);
  expect(document.querySelector('[data-testid="git-confirm-dialog"]')).toBeNull();
});

it("rolls back with the requests git ran for that list after the confirmation", async () => {
  host.entries = DISCARD.rollback.entries;
  await openCommitWindow("staged:added.txt");
  const dialog = await rollbackEverything(DISCARD.rollback);
  await click(dialogButton(dialog, "되돌리기"));
  await vi.waitFor(() => expect(calls("git.discard")).toHaveLength(1));
  expect(calls("git.unstage").map((c) => c.params.paths)).toEqual([DISCARD.rollback.unstage]);
  expect(calls("git.discard")[0]!.params).toMatchObject({
    paths: DISCARD.rollback.discard,
    confirmed: true,
  });
});

it("rolls back only the work tree when only the changes group is chosen, keeping intent-to-add files", async () => {
  const fx = DISCARD.rollbackChanges;
  host.entries = fx.entries;
  await openCommitWindow("changes:ita.txt");
  const dialog = await rollbackEverything(fx, false);
  expectDanger(dialog);
  expect(lostLines(dialog)).toEqual(fx.lost);
  expectLostLike(dialog, fx);
  await click(dialogButton(dialog, "되돌리기"));
  await vi.waitFor(() => expect(calls("git.discard")).toHaveLength(1));
  expect(calls("git.unstage")).toHaveLength(0);
  expect(fx.unstage).toEqual([]);
  expect(calls("git.discard")[0]!.params).toMatchObject({ paths: fx.discard, confirmed: true });
});

it("shows a submodule as left alone, not lost, the way git restore leaves it", async () => {
  const fx = DISCARD.rollbackSubmodule;
  host.entries = fx.entries;
  host.dirs = new Set(["sm"]);
  await openCommitWindow("staged:sm");
  const dialog = await rollbackEverything(fx);
  expect(lostLines(dialog)).toEqual(["plain.txt"]);
  expectLostLike(dialog, fx);
  await click(dialogButton(dialog, "되돌리기"));
  await vi.waitFor(() => expect(calls("git.discard")).toHaveLength(1));
  expect(calls("git.unstage").map((c) => c.params.paths)).toEqual([fx.unstage]);
  expect(calls("git.discard")[0]!.params).toMatchObject({ paths: fx.discard, confirmed: true });
});

it("asks again with the new list when the state changes while the list is shown (review)", async () => {
  // 리뷰의 재현: 「지우지 않고 그대로 둔다」 의 새 파일 n 을 보는 동안 터미널에서 커밋하고 n 을 고친다.
  host.entries = [{ path: "n", origPath: null, staged: "added", unstaged: null, conflict: null }];
  await openCommitWindow("staged:n");
  await click(row("staged:n")!);
  await act(async () => {
    windowRegistry().run("git.rollback");
  });
  let dialog = await confirmDialog();
  expect(bodySection(dialog, "지우지 않고")).toEqual(["n"]);
  expect(lostLines(dialog)).toEqual([]);
  host.entries = [
    { path: "n", origPath: null, staged: null, unstaged: "modified", conflict: null },
  ];
  await click(dialogButton(dialog, "되돌리기"));
  // 보였던 목록과 다르다 — 부르지 않고 새 목록으로 다시 묻는다.
  dialog = await vi.waitFor(() => {
    const d = document.querySelector<HTMLElement>('[data-testid="git-confirm-dialog"]');
    expect(d?.textContent).toContain("그사이 상태가 바뀌었다");
    return d!;
  });
  expect(calls("git.unstage")).toHaveLength(0);
  expect(calls("git.discard")).toHaveLength(0);
  expectDanger(dialog);
  expect(lostLines(dialog)).toEqual(["n"]);
  expect(bodySection(dialog, "지우지 않고")).toEqual([]);
  await click(dialogButton(dialog, "되돌리기"));
  await vi.waitFor(() => expect(calls("git.discard")).toHaveLength(1));
  // 커밋된 뒤라 스테이징을 풀 것이 없다 — 작업 트리의 새 편집을 버린다는 것을 보인 뒤에만.
  expect(calls("git.unstage")).toHaveLength(0);
  expect(calls("git.discard")[0]!.params).toMatchObject({ paths: ["n"], confirmed: true });
});

it("shows the hunk's lines before reverting it and sends the request git ran", async () => {
  const step = DISCARD.hunk;
  host.fixture = { steps: [step], staged: step };
  host.entries = [MODIFIED];
  await openCommitWindow();
  await click(row("changes:src/app.rs")!);
  const revert = () => document.querySelector<HTMLElement>(".cm-merge-b [data-stage-revert]");
  await vi.waitFor(() => expect(revert()).toBeTruthy());
  await click(revert()!);
  let dialog = await confirmDialog();
  expectDanger(dialog);
  const shown = dialog
    .querySelector('[data-testid="git-confirm-detail"]')!
    .textContent!.split("\n");
  const { hunk, lines } = step.request.hunks[0] as {
    hunk: { lines: { kind: string; text: string }[] };
    lines: number[];
  };
  expect(shown).toEqual(
    lines.map((i) => `${hunk.lines[i]!.kind === "added" ? "+" : "-"}${hunk.lines[i]!.text}`),
  );
  // 그 줄이 git 이 실제로 바꾼 것이다: + 줄은 파일에서 지워지고 - 줄이 돌아왔다.
  const before = step.workTree.split("\n");
  const after = step.after.split("\n");
  for (const line of shown) {
    const text = line.slice(1);
    if (line.startsWith("+"))
      expect([before.includes(text), after.includes(text)]).toEqual([true, false]);
    else expect([before.includes(text), after.includes(text)]).toEqual([false, true]);
  }
  await click(dialogButton(dialog, "취소"));
  await settle(50);
  expect(calls("git.discardLines")).toHaveLength(0);

  await click(revert()!);
  dialog = await confirmDialog();
  await click(dialogButton(dialog, "되돌리기"));
  await vi.waitFor(() => expect(calls("git.discardLines")).toHaveLength(1));
  const { params } = calls("git.discardLines")[0]!;
  expect({ path: params.path, hunks: params.hunks }).toEqual(step.request);
  expect(params).toMatchObject({ confirmed: true, ignoreWhitespace: false });
});

it("names the stash before dropping it and drops the one git dropped", async () => {
  host.stashes = DISCARD.stash.stashes;
  await openCommitWindow();
  const { index, commit } = DISCARD.stash.request;
  const drop = () =>
    document.querySelector<HTMLElement>(
      `[data-commit-stash="${index}"] [data-commit-stash-action="drop"]`,
    );
  await vi.waitFor(() => expect(drop()).not.toBeNull());
  await click(drop()!);
  let dialog = await confirmDialog();
  expectDanger(dialog);
  const message = DISCARD.stash.stashes.find((s) => s.index === index)!.message;
  expect(dialog.textContent).toContain(`stash@{${index}} ${message}`);
  await click(dialogButton(dialog, "취소"));
  await settle(50);
  expect(calls("git.stash.drop")).toHaveLength(0);

  host.stashes = DISCARD.stash.after;
  await click(drop()!);
  dialog = await confirmDialog();
  await click(dialogButton(dialog, "버리기"));
  await vi.waitFor(() => expect(calls("git.stash.drop")).toHaveLength(1));
  expect(calls("git.stash.drop")[0]!.params).toMatchObject({ index, commit, confirmed: true });
  // 다시 읽은 목록은 git 이 남긴 것이다.
  await vi.waitFor(() => expect(drop()).toBeNull());
});

it("offers [Undo] after amend through git.undo.offer and resets soft to the previous commit", async () => {
  host.entries = [{ ...MODIFIED, staged: "modified" }];
  host.moved = DISCARD.amend.moved;
  await openCommitWindow();
  await click(document.querySelector("[data-commit-amend]")!);
  await vi.waitFor(() => expect(messageBox()!.value).toBe(host.headMessage));
  await act(async () => {
    windowRegistry().run("git.commit.run");
  });
  await vi.waitFor(() => expect(calls("git.commit")).toHaveLength(1));
  const { previous, current } = DISCARD.amend.moved;
  const undo = await toastButton(
    `main: ${previous.slice(0, 8)} → ${current.slice(0, 8)}`,
    "되돌리기",
  );
  expect(calls("git.reset")).toHaveLength(0);
  await click(undo);
  await vi.waitFor(() => expect(calls("git.reset")).toHaveLength(1));
  // soft 라 미리보기 · 확인 없이 — 그 git 으로 돌린 요청 그대로다(HEAD 는 이전 커밋, index 는 amend 직전 그대로).
  expect(calls("git.reset")[0]!.params).toEqual({
    projectId: "p1",
    worktree: null,
    ...DISCARD.amend.request,
  });
  expect(calls("git.resetPreview")).toHaveLength(0);
});
