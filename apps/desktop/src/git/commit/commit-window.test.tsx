// @vitest-environment happy-dom
// 커밋 툴 윈도우(13-m2-plan M2-16 완료 기준)를 프로젝트 창(App kind="project")과 가짜 Host 로 본다. 시나리오 2 의 답은 진짜 pw-git ·
// git 의 출력이다(fixtures/stage-commit.json — crates/pw-git `commit::tests::window_fixture` 가 만들고, 그 JSON 이 지금 pw-git · git 의
// 출력과 같은지 본다): 체크를 눌러 창이 보낸 `git.stageLines` 요청이 그 JSON 의 요청과 같으면, 그 요청을 진짜 git 으로 돌린 뒤의
// `git show --stat` 이 JSON 의 `stat` 이다.
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
  emit: (_topic: string, _payload: unknown) => {},
  fixture: undefined as unknown as typeof FIXTURE,
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
            return reply({ operation: null });
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
            return reply({ template: host.template });
          case "git.stash.list":
            return reply({ workTree: "/repo", stashes: [] });
          case "git.stageLines":
            host.step++;
            return reply({ workTree: "/repo" });
          case "git.stage":
          case "git.unstage":
            return reply({ workTree: "/repo" });
          case "git.discard":
            return reply({ workTree: "/repo", untracked: [] });
          case "git.commit":
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
              moved: {
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
  host.fixture = FIXTURE;
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

/** ⌘K 로 커밋 툴 윈도우를 연다(메시지 칸에 포커스가 든다). */
async function openCommitWindow() {
  await mount();
  await act(async () => {
    windowRegistry().run("git.commit");
  });
  await vi.waitFor(() => expect(row("changes:src/app.rs")).not.toBeNull());
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
  const confirm = await vi.waitFor(() => {
    const b = [...document.querySelectorAll("button")].find((x) => x.textContent === "되돌리기");
    expect(b).toBeDefined();
    return b!;
  });
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
