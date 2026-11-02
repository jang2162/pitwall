// @vitest-environment happy-dom
// 언어 서버 상태 항목 · 상태 팝업 · 탭 위 배너 · 확인 다이얼로그(15-m4-plan M4-17)를 프로젝트 창(App kind="project")과 가짜 Host 로
// 본다 — 상태마다 글자 · 표지가 다르고(색만이 아니다), 「이 프로젝트에서 끄기」 · 「묻지 않기」 는 개인 설정 쓰기(`lsp.server.setLocal`)
// 로만 가고, 확인 다이얼로그를 지나지 않은 확인 값으로 설치하지 않고(처음 포커스 [취소]), 설치 뒤 그 서버를 다시 띄운다
// (`lsp.server.restart` — 허가는 Host 가 다시 본다). `runtimeMissing` 배너에는 설치 단추가 없다. 창 틀(가짜 Host · 마운트)은
// lsp-window.test.tsx 와 같다. Host 쪽(개인 파일 · 신뢰 뒤 기동 · 허가)은 crates/pw-lsp/tests/m4_17_*.rs.
//
// CodeMirror 는 불러올 때 macOS 인지 정한다(navigator.platform) — 그래서 불러오기 전에 Mac 으로 둔다.

import { EditorView } from "@codemirror/view";
import type {
  LspInstanceState,
  LspInstanceStatus,
  ConfigView,
  JsonValue,
  FsRead,
  FsStat,
  HostInfo,
  KeymapFile,
  LangDetectResult,
  LangList,
  Project,
  ProjectTrust,
} from "@pitwall/rpc-client";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import type { KeymapNative } from "../keymap/native";
import { activeWorktreeStore } from "../open-with/active-worktree";
import { projectOpenInbox } from "../project-open";
import { createThemeStore } from "../theme/theme-store";
import { act, endTest, it } from "../test-support/ended-test";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { fakeSyntaxServer } from "../editor/syntax-testing";
import { bundledTranslator } from "../i18n/testing";
import { toolsStrings } from "../tools/strings";
import { fakeLspServer } from "./lsp-testing";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

vi.hoisted(() => {
  Object.defineProperty(navigator, "platform", { value: "MacIntel", configurable: true });
});

const host = vi.hoisted(() => ({
  files: new Map<string, string>(),
  /** 전역 settings.toml 의 값(`config.get`). */
  settings: {} as JsonValue,
  subscribers: [] as {
    send(m: object): void;
    id: number;
    topics: string[];
    projectId?: string;
  }[],
  /** 구문 분석 서비스(`syntax.*`). 없으면 그 메서드가 없는 Host 다(강조하지 않는다). */
  syntax: undefined as ReturnType<typeof fakeSyntaxServer> | undefined,
  /** 서버 관리자(`lsp.document.*`). */
  lsp: undefined as ReturnType<typeof fakeLspServer> | undefined,
  /** 크기 등급을 바꿀 파일(5 MiB 초과의 읽기 전용 등). */
  sizeClass: new Map<string, "editable" | "readOnly">(),
  /** 외부로 연 주소(`openUrl`). */
  opened: [] as string[],
  /** 이 창이 부른 메서드(문서 동기화 · 구독 밖). */
  calls: [] as { method: string; params: Record<string, unknown> }[],
  /** 언어(Linguist) → 그 문서를 맡는 인스턴스. */
  instances: {} as Record<string, import("@pitwall/rpc-client").LspInstanceStatus[]>,
  /** 서버 정의(`lsp.servers.list`). */
  servers: [] as Record<string, unknown>[],
  /** 설치기가 확인을 바라는 값(없으면 묻지 않고 설치한다). */
  consent: undefined as import("@pitwall/rpc-client").ToolConfirmRequired | undefined,
  /** 프로젝트 신뢰. */
  trust: "trusted" as "trusted" | "untrusted",
  /** `tools.updates` 의 항목. */
  updates: [] as {
    name: string;
    installed: string;
    available: string;
    confirmRequired: boolean;
    checksum: "pinned" | "none";
  }[],
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: async (url: string) => void host.opened.push(url),
}));

const LANGUAGES: LangList = {
  languages: [
    { name: "Python", popular: true, lineComment: "#" },
    { name: "Rust", popular: true, lineComment: "//" },
    { name: "COBOL", popular: false, lineComment: null },
    { name: "Text", popular: false, lineComment: null },
  ],
};

/** 이 행의 Host 메서드. 처리했으면 참. */
function handle(
  method: string,
  p: Record<string, unknown>,
  send: (m: object) => void,
  id: number | undefined,
): boolean {
  const reply = (result: unknown) => send({ id, result });
  const fail = (code: number, message: string, data?: unknown) =>
    send({ id, error: { code, message, data } });
  const record = () => host.calls.push({ method, params: p });
  switch (method) {
    case "lsp.status.document": {
      const language = p.language as string;
      const instances = host.instances[language] ?? [];
      reply({
        language: language.toLowerCase(),
        servers:
          language === "Python"
            ? ["basedpyright", "ruff"]
            : language === "Rust"
              ? ["rust-analyzer"]
              : [],
        instances,
      });
      return true;
    }
    case "lsp.servers.list":
      reply({ servers: host.servers, trusted: true, problem: null, warnings: [] });
      return true;
    case "lsp.server.setLocal": {
      record();
      const server = host.servers.find((s) => s.id === p.server);
      if (server && p.key === "installPrompt") server.installPrompt = p.value;
      if (p.key === "enabled")
        for (const list of Object.values(host.instances))
          for (const i of list) if (i.server === p.server) i.state = { kind: "disabled" };
      reply({ file: "/work/demo/.pitwall/project.local.toml" });
      return true;
    }
    case "lsp.server.restart":
    case "lsp.server.stop": {
      record();
      for (const list of Object.values(host.instances))
        for (const i of list)
          if (i.server === p.server && i.root === p.root)
            i.state = method === "lsp.server.stop" ? { kind: "stopped" } : { kind: "ready" };
      reply(null);
      return true;
    }
    case "lsp.server.log":
      record();
      reply({ text: "stderr <b>not bold</b>\n", truncated: false, file: "/state/logs/lsp/x.log" });
      return true;
    case "tools.registry.get":
      reply({
        summary: {
          name: p.name,
          description: "<img src=x onerror=alert(1)> Python type checker",
          version: "1.40.2",
          licenses: ["MIT"],
          languages: ["Python"],
          categories: ["LSP"],
          bundled: true,
          deprecated: false,
          unsupported: null,
        },
        homepage: "javascript:alert(1)",
        sourceId: "pkg:pypi/basedpyright@1.40.2",
        deprecation: null,
        resolved: {
          name: p.name,
          version: "1.40.2",
          kind: "pypi",
          target: "darwin_arm64",
          source: { kind: "pypi", package: { name: "basedpyright", version: "1.40.2" }, extra: [] },
          bin: {},
          share: {},
          opt: {},
          overridden: false,
        },
        pinned: [],
        checksum: "pinned",
        registryVersion: "r1",
      });
      return true;
    case "tools.install": {
      record();
      const c = p.confirmed as {
        version: string;
        licenses: string[];
        sourceDomains: string[];
      } | null;
      const want = host.consent;
      const covered =
        !want ||
        (c !== null &&
          c.version === want.version &&
          JSON.stringify(c.licenses) === JSON.stringify(want.licenses) &&
          JSON.stringify(c.sourceDomains) === JSON.stringify(want.sourceDomains));
      if (!covered) {
        fail(4001, "confirmation required", { ...want, rejected: c ? "changed" : null });
        return true;
      }
      host.consent = undefined;
      reply({
        name: p.name,
        version: "1.40.2",
        previous: null,
        outcome: "installed",
        checksum: "pinned",
        taskId: 7,
        joined: false,
      });
      return true;
    }
    case "tools.updates":
      record();
      reply({ items: host.updates, registryVersion: "r1" });
      return true;
  }
  return false;
}

vi.mock("../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../keymap/default-keymap");
  const { i18nState } = await import("../i18n/testing");
  return {
    connectHost: async () => {
      const [client, server] = pair();
      let next = 1;
      const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const reply = (result: unknown) => send({ id: m.id, result });
        const path = (m.params as { path?: string } | undefined)?.path ?? "";
        if (m.method.startsWith("syntax.") && host.syntax) {
          const { source } = host.syntax;
          const run = {
            "syntax.open": source.open,
            "syntax.edit": source.edit,
            "syntax.highlights": source.highlights,
            "syntax.close": source.close,
          }[m.method] as (p: unknown) => Promise<unknown>;
          return void run(m.params).then(reply, (e: { code?: number; message: string }) =>
            send({ id: m.id, error: { code: e.code ?? -32603, message: e.message } }),
          );
        }
        if (m.method.startsWith("lsp.document.") && host.lsp) {
          const { source } = host.lsp;
          const run = {
            "lsp.document.open": source.open,
            "lsp.document.change": source.change,
            "lsp.document.save": source.save,
            "lsp.document.close": source.close,
            "lsp.document.capabilities": source.capabilities,
          }[m.method] as (p: unknown) => Promise<unknown>;
          return void run(m.params).then(reply, (e: { code?: number; message: string }) =>
            send({ id: m.id, error: { code: e.code ?? -32603, message: e.message } }),
          );
        }
        const p = (m.params ?? {}) as Record<string, unknown>;
        const handled = handle(m.method, p, send, m.id);
        if (handled) return;
        switch (m.method) {
          case "i18n.get":
            return reply(i18nState("ko"));
          case "config.get":
            return reply({
              snapshot: {
                files: [{ id: "settings", value: host.settings, layers: [] }],
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
          // 상태바의 브랜치 항목(src/git/branch-state.ts)이 묻는다 — 이 시험은 저장소 없음으로 답한다.
          case "git.refs":
            return reply({
              workTree: null,
              head: null,
              branches: [],
              remoteBranches: [],
              tags: [],
              unsupported: null,
              broken: [],
            });
          case "git.operation":
            return reply({ operation: null });
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
            return reply({
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
            } satisfies Project);
          case "project.trust":
            return reply({
              projectId: "p1",
              ownerId: "p1",
              state: host.trust,
            } satisfies ProjectTrust);
          case "project.storage":
            return reply({ kind: "ok", path: "/data/state.db" });
          case "project.claimTrustPrompt":
            return reply(false);
          case "project.setTrust":
            host.trust = (m.params as { trusted: boolean }).trusted ? "trusted" : "untrusted";
            return reply(null);
          case "fs.stat":
            return reply({
              path,
              kind: "file",
              target: null,
              size: host.files.get(path)!.length,
              mtimeMs: 1,
              readonly: false,
              sizeClass: host.sizeClass.get(path) ?? "editable",
            } satisfies FsStat);
          case "fs.read":
            return reply({
              path,
              size: host.files.get(path)!.length,
              mtimeMs: 1,
              offset: 0,
              eof: true,
              version: "1",
              content: { kind: "text", text: host.files.get(path)! },
              judgment: {
                binary: false,
                encoding: "UTF-8",
                lossy: false,
                bom: null,
                lineEnding: "lf",
                longLines: false,
                sizeClass: host.sizeClass.get(path) ?? "editable",
                editable: (host.sizeClass.get(path) ?? "editable") === "editable",
                highlight: true,
              },
            } satisfies FsRead);
          case "lang.detect": {
            const { files } = m.params as { files: { path: string }[] };
            return reply({
              files: files.map((f) => ({
                language: f.path.endsWith(".rs")
                  ? "Rust"
                  : f.path.endsWith(".py")
                    ? "Python"
                    : null,
                source: "extension",
                vendored: false,
                generated: false,
                documentation: false,
              })),
            } satisfies LangDetectResult);
          }
          case "lang.list":
            return reply(LANGUAGES);
          case "events.subscribe": {
            const id = next++;
            const p = m.params as { topics: string[]; projectId?: string };
            host.subscribers.push({ send, id, topics: p.topics, projectId: p.projectId });
            return reply({ subscriptionId: id });
          }
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

/** 네이티브 메뉴(src-tauri/src/menu.rs): 마지막으로 받은 accelerator 와 메뉴에서 고른 액션을 보내는 자리. */
const menu = {
  accelerators: {} as Record<string, string | null>,
  choose: undefined as ((action: string) => void) | undefined,
};

const native = {
  syncMenu: async (accelerators) => {
    menu.accelerators = accelerators;
  },
  onMenuAction: (listener) => {
    menu.choose = listener;
    return () => {
      if (menu.choose === listener) menu.choose = undefined;
    };
  },
  openSettings: async () => {},
  closeWindow: async () => {},
  guardClose: () => () => {},
} satisfies KeymapNative;

/** 낱말 `foo` 셋. */
const WORDS = "/work/demo/words.py";
const MAIN = "/work/demo/src/main.rs";
/** 크기 등급이 읽기 전용(5 MiB 초과로 판정). */
const BIG = "/work/demo/big.py";
/** 언어가 없는 파일(플레인 텍스트). */
const NOTES = "/work/demo/notes.txt";

let root: Root | undefined;

function server(id: string, mason: string | null): Record<string, unknown> {
  return {
    id,
    extension: "dev.pitwall.x",
    languages: [],
    mason,
    command: id,
    args: [],
    env: {},
    rootMarkers: [],
    requires: [],
    disabledFeatures: [],
    enabled: true,
    settings: {},
    initializationOptions: {},
    resolution: {
      kind: "found",
      path: `/tools/bin/${id}`,
      step: "installed",
      version: "1.0.0",
      skipped: [],
    },
    installPrompt: true,
  };
}

function useUserAgent(ua: string) {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(ua);
}

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  useUserAgent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");
  vi.spyOn(console, "info").mockImplementation(() => {});
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.files = new Map([
    [WORDS, "foo = 1\nbar = foo\nfoobar = foo\n"],
    [MAIN, "fn main() {}\n"],
    [BIG, "x = 1\n"],
    [NOTES, "hello\n"],
  ]);
  host.settings = {};
  host.subscribers = [];
  host.syntax = undefined;
  host.lsp = fakeLspServer();
  host.sizeClass = new Map([[BIG, "readOnly"]]);
  host.opened = [];
  host.calls = [];
  host.instances = {};
  host.servers = [
    server("basedpyright", "basedpyright"),
    server("ruff", "ruff"),
    server("rust-analyzer", "rust-analyzer"),
  ];
  host.consent = undefined;
  host.trust = "trusted";
  host.updates = [];
  activeWorktreeStore.setState({ top: undefined });
  localStorage.clear();
  // happy-dom 은 DOM 선택을 바꾸는 그 자리에서 `selectionchange` 를 동기로 보낸다(브라우저는 나중에 보낸다). 줄 수가 바뀌는 편집이면
  // CodeMirror 가 자기 업데이트 도중에 자기가 둔 선택의 이벤트를 받아 「update in progress」 로 멈춘다. 브라우저에서는 그 이벤트가
  // 업데이트 뒤에 와 아무것도 바꾸지 않으므로, 업데이트 중에 온 것은 버린다(CodeMirror 의 `updateState`: 0 = 쉼).
  const dispatch = document.dispatchEvent.bind(document);
  vi.spyOn(document, "dispatchEvent").mockImplementation((e) => {
    const updating = [...document.querySelectorAll<HTMLElement>(".cm-editor")].some(
      (dom) =>
        (EditorView.findFromDOM(dom) as unknown as { updateState: number }).updateState !== 0,
    );
    return e.type === "selectionchange" && updating ? true : dispatch(e);
  });
});

afterEach(async () => {
  await endTest();
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 20)));

async function mount() {
  const element = document.createElement("div");
  element.style.width = "1280px";
  element.style.height = "800px";
  document.body.append(element);
  const store = createThemeStore({
    system: { current: () => "light", subscribe: () => () => {} },
    storage: { read: () => undefined, write: () => {} },
    theme: parseTheme(defaultTheme),
    apply: () => {},
  });
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <ThemeStoreProvider store={store}>
        <App kind="project" native={native} />
      </ThemeStoreProvider>,
    ),
  );
  await settle();
}

async function open(file: string, line?: number, col?: number) {
  await act(async () => projectOpenInbox.push({ file, line, col }));
  await settle();
}

/** 프로젝트 범위 이벤트를 구독한 연결에 보낸다. */
async function emit(topic: string, payload: unknown) {
  await act(async () => {
    for (const sub of host.subscribers)
      if (sub.topics.includes(topic))
        sub.send({
          method: "events.event",
          params: { subscriptionId: sub.id, projectId: "p1", topic, payload },
        });
  });
  await settle();
}

const T = toolsStrings(bundledTranslator("ko"));
const ROOT = "/work/demo";

const item = () => document.querySelector<HTMLElement>('[data-testid="status-lsp"]');
const banner = () =>
  document.querySelector<HTMLElement>('.dv-active-group [data-testid="lsp-banner"]');
const bannerButtons = () =>
  [...(banner()?.querySelectorAll<HTMLButtonElement>("button") ?? [])].map((b) => b.dataset.action);
const dialog = () => document.querySelector<HTMLElement>('[data-testid="tool-confirm-dialog"]');
const called = (method: string) =>
  host.calls.filter((c) => c.method === method).map((c) => c.params);

function instance(
  server: string,
  state: LspInstanceState,
  progress: LspInstanceStatus["progress"] = null,
): LspInstanceStatus {
  return { server, root: ROOT, state, progress };
}

async function click(el: Element | null | undefined) {
  expect(el).toBeTruthy();
  await act(async () => (el as HTMLElement).click());
  await settle();
}

/** 상태가 바뀌었다는 이벤트(Host 의 `lsp.status`). 창은 문서의 상태를 다시 읽는다. */
const changed = () => emit("lsp.status", { projectId: "p1", instances: [] });

it("shows each server state in its own words and mark, not colour alone", async () => {
  await mount();
  host.instances.Python = [instance("basedpyright", { kind: "starting" })];
  await open(WORDS, 1, 1);
  await settle();
  const states: [LspInstanceState, LspInstanceStatus["progress"]][] = [
    [{ kind: "starting" }, null],
    [{ kind: "ready" }, null],
    [{ kind: "ready" }, { title: "인덱싱", message: null, percentage: 42 }],
    [{ kind: "failed", message: "boom" }, null],
    [{ kind: "crashed" }, null],
    [{ kind: "restartNeeded" }, null],
    [{ kind: "restartSuggested" }, null],
    [{ kind: "notInstalled", mason: "basedpyright" }, null],
    [{ kind: "runtimeMissing", tool: "python", needed: ">=3.10" }, null],
    [{ kind: "untrusted" }, null],
    [{ kind: "disabled" }, null],
    [{ kind: "stopped" }, null],
  ];
  const seen: [string, string, string][] = [];
  for (const [state, progress] of states) {
    host.instances.Python = [instance("basedpyright", state, progress)];
    await changed();
    const el = item()!;
    expect(el.textContent).toContain("basedpyright");
    seen.push([
      el.dataset.state!,
      el.querySelector('[data-testid="status-lsp-label"]')!.textContent!,
      el.querySelector("svg")!.getAttribute("class") ?? "",
    ]);
  }
  expect(seen.map(([kind]) => kind)).toEqual([
    "starting",
    "ready",
    "progress",
    "failed",
    "crashed",
    "restartNeeded",
    "restartSuggested",
    "notInstalled",
    "runtimeMissing",
    "untrusted",
    "disabled",
    "stopped",
  ]);
  // 글자는 상태마다 다르다(다시 시작 둘도). 진행은 %를 싣는다.
  expect(new Set(seen.map(([, label]) => label)).size).toBe(seen.length);
  expect(seen[2]![1]).toBe("인덱싱 42%");
  // 표지(아이콘)는 준비 · 진행 · 오류 · 설치 필요 · 런타임 없음 · 미신뢰 · 꺼짐 · 멈춤이 서로 다르다.
  const icon = (kind: string) => seen.find(([k]) => k === kind)![2];
  const marks = [
    "ready",
    "progress",
    "failed",
    "notInstalled",
    "runtimeMissing",
    "untrusted",
    "disabled",
    "stopped",
    "starting",
  ].map(icon);
  expect(new Set(marks).size).toBe(marks.length);
  // 같은 문서를 맡는 서버가 여럿이면 앞의 것과 나머지 수.
  host.instances.Python = [
    instance("basedpyright", { kind: "ready" }),
    instance("ruff", { kind: "ready" }),
  ];
  await changed();
  expect(item()!.textContent).toContain("+1");
});

it("turns a server off in this project only through the personal setting", async () => {
  await mount();
  host.instances.Python = [instance("basedpyright", { kind: "ready" })];
  await open(WORDS, 1, 1);
  await settle();
  await click(item());
  const popup = document.querySelector('[data-testid="lsp-status-popup"]')!;
  expect(popup.textContent).toContain(ROOT);
  expect(popup.textContent).toContain(T.popup.foundIn.installed);
  await click(popup.querySelector('[data-testid="lsp-status-disable"]'));
  expect(called("lsp.server.setLocal")).toEqual([
    { projectId: "p1", worktree: null, server: "basedpyright", key: "enabled", value: false },
  ]);
  // 그 밖에 설정 · 파일을 쓰는 RPC 는 없다(커밋되는 `.pitwall/project.toml` 에 쓰는 길이 창에 없다).
  expect(host.calls.map((c) => c.method).filter((m) => /write|set|save|fs\./.test(m))).toEqual([
    "lsp.server.setLocal",
  ]);
  expect(item()!.dataset.state).toBe("disabled");
  // 쓴 개인 파일을 알린다.
  expect(document.body.textContent).toContain("project.local.toml");
});

it("restarts, stops and shows the log of an instance from the popup", async () => {
  await mount();
  host.instances.Python = [instance("basedpyright", { kind: "restartNeeded" })];
  await open(WORDS, 1, 1);
  await settle();
  await click(item());
  await click(document.querySelector('[data-testid="lsp-status-restart"]'));
  expect(called("lsp.server.restart")).toEqual([
    { projectId: "p1", worktree: null, server: "basedpyright", root: ROOT },
  ]);
  expect(item()!.dataset.state).toBe("ready");
  await click(document.querySelector('[data-testid="lsp-status-stop"]'));
  expect(item()!.dataset.state).toBe("stopped");
  await click(document.querySelector('[data-testid="lsp-status-log"]'));
  const log = document.querySelector('[data-testid="lsp-status-log-text"]')!;
  // 로그는 글로만 그린다.
  expect(log.textContent).toContain("<b>not bold</b>");
  expect(log.querySelector("b")).toBeNull();
});

it("installs a missing server only through the confirmation dialog, then starts it (notInstalled → ready)", async () => {
  await mount();
  host.instances.Python = [
    instance("basedpyright", { kind: "notInstalled", mason: "basedpyright" }),
  ];
  host.consent = {
    name: "basedpyright",
    version: "1.40.2",
    licenses: ["MIT"],
    sourceDomains: ["files.pythonhosted.org", "pypi.org"],
    size: 14_365_000,
    checksum: "pinned",
    note: null,
    rejected: null,
  };
  await open(WORDS, 1, 1);
  await settle();
  expect(banner()!.dataset.kind).toBe("notInstalled");
  expect(banner()!.textContent).toContain("Python 언어 서버 basedpyright(MIT)가 없다");
  expect(bannerButtons()).toEqual(["install", "settings", "dontAsk"]);

  await click(banner()!.querySelector('[data-action="install"]'));
  // 설치기가 확인을 바란다 — 다이얼로그가 뜨고, 그때까지 확인 값을 실은 설치는 없다.
  expect(dialog()).toBeTruthy();
  expect(called("tools.install")).toEqual([
    { name: "basedpyright", version: null, confirmed: null },
  ]);
  // 처음 포커스는 [취소].
  expect(document.activeElement?.getAttribute("data-testid")).toBe("tool-confirm-cancel");
  expect(dialog()!.textContent).toContain("files.pythonhosted.org");
  expect(dialog()!.querySelector('[data-testid="tool-confirm-size"]')!.textContent).toBe("14 MiB");
  expect(
    dialog()!.querySelector<HTMLElement>('[data-testid="tool-confirm-integrity"]')!.dataset
      .checksum,
  ).toBe("pinned");
  // registry 의 글은 글로만: 설명의 태그는 요소가 되지 않고, http(s) 가 아닌 홈페이지는 링크가 아니다.
  expect(
    dialog()!.querySelector('[data-testid="tool-confirm-description"]')!.textContent,
  ).toContain("<img");
  expect(dialog()!.querySelector("img")).toBeNull();
  expect(dialog()!.querySelector('a[href^="javascript"]')).toBeNull();
  expect(
    dialog()!.querySelector<HTMLAnchorElement>('[data-testid="tool-confirm-license"] a')!.href,
  ).toBe("https://spdx.org/licenses/MIT.html");

  await click(dialog()!.querySelector('[data-testid="tool-confirm-accept"]'));
  await settle();
  expect(called("tools.install")).toEqual([
    { name: "basedpyright", version: null, confirmed: null },
    {
      name: "basedpyright",
      version: null,
      confirmed: {
        version: "1.40.2",
        licenses: ["MIT"],
        sourceDomains: ["files.pythonhosted.org", "pypi.org"],
      },
    },
  ]);
  // 설치가 끝나면 그 인스턴스를 다시 띄운다(허가는 Host 가 다시 본다) — 상태가 「준비」.
  expect(called("lsp.server.restart")).toEqual([
    { projectId: "p1", worktree: null, server: "basedpyright", root: ROOT },
  ]);
  expect(item()!.dataset.state).toBe("ready");
  expect(banner()!.hidden).toBe(true);
});

it("calls nothing more when the confirmation is cancelled", async () => {
  await mount();
  host.instances.Python = [
    instance("basedpyright", { kind: "notInstalled", mason: "basedpyright" }),
  ];
  host.consent = {
    name: "basedpyright",
    version: "1.40.2",
    licenses: ["MIT"],
    sourceDomains: ["pypi.org"],
    size: null,
    checksum: "none",
    note: null,
    rejected: null,
  };
  await open(WORDS, 1, 1);
  await settle();
  await click(banner()!.querySelector('[data-action="install"]'));
  expect(
    dialog()!.querySelector<HTMLElement>('[data-testid="tool-confirm-integrity"]')!.dataset
      .checksum,
  ).toBe("none");
  await click(dialog()!.querySelector('[data-testid="tool-confirm-cancel"]'));
  expect(dialog()).toBeNull();
  expect(called("tools.install")).toHaveLength(1);
  expect(called("lsp.server.restart")).toEqual([]);
  expect(banner()!.dataset.kind).toBe("notInstalled");
});

it("asks again when the license changed while the dialog was open", async () => {
  await mount();
  host.instances.Python = [
    instance("basedpyright", { kind: "notInstalled", mason: "basedpyright" }),
  ];
  const first = {
    name: "basedpyright",
    version: "1.40.2",
    licenses: ["MIT"],
    sourceDomains: ["pypi.org"],
    size: null,
    checksum: "pinned" as const,
    note: null,
    rejected: null,
  };
  host.consent = first;
  await open(WORDS, 1, 1);
  await settle();
  await click(banner()!.querySelector('[data-action="install"]'));
  host.consent = { ...first, licenses: ["GPL-3.0-only"] };
  await click(dialog()!.querySelector('[data-testid="tool-confirm-accept"]'));
  // 다시 묻는다 — 바뀐 라이선스 · 「바뀌었다」 안내, 처음 포커스 [취소].
  expect(dialog()!.textContent).toContain("GPL-3.0-only");
  expect(dialog()!.textContent).toContain(T.confirm.changed);
  expect(document.activeElement?.getAttribute("data-testid")).toBe("tool-confirm-cancel");
  expect(called("lsp.server.restart")).toEqual([]);
  await click(dialog()!.querySelector('[data-testid="tool-confirm-accept"]'));
  expect(called("tools.install")).toHaveLength(3);
  expect(called("lsp.server.restart")).toHaveLength(1);
});

it("shows the server runtime to install with mise and no install button for runtimeMissing", async () => {
  await mount();
  host.instances.Python = [
    instance("basedpyright", { kind: "runtimeMissing", tool: "python", needed: ">=3.10" }),
  ];
  await open(WORDS, 1, 1);
  await settle();
  expect(banner()!.dataset.kind).toBe("runtimeMissing");
  expect(banner()!.textContent).toBe(
    "서버용 Python >=3.10 가 필요하다 — `mise install python@3.10`(프로젝트 고정은 바꾸지 않는다)",
  );
  expect(bannerButtons()).toEqual([]);
  await click(item());
  const popup = document.querySelector('[data-testid="lsp-status-popup"]')!;
  expect(popup.querySelector('[data-testid="lsp-status-install"]')).toBeNull();
  expect(popup.textContent).toContain("mise install python@3.10");
  expect(called("tools.install")).toEqual([]);
});

it("stops asking in this project only through the personal setting", async () => {
  await mount();
  host.instances.Python = [
    instance("basedpyright", { kind: "notInstalled", mason: "basedpyright" }),
  ];
  await open(WORDS, 1, 1);
  await settle();
  await click(banner()!.querySelector('[data-action="dontAsk"]'));
  expect(called("lsp.server.setLocal")).toEqual([
    { projectId: "p1", worktree: null, server: "basedpyright", key: "installPrompt", value: false },
  ]);
  expect(host.calls.map((c) => c.method).filter((m) => /write|set|save|fs\./.test(m))).toEqual([
    "lsp.server.setLocal",
  ]);
  expect(banner()!.hidden).toBe(true);
  // 상태바는 그대로 「설치 필요」 다(설치는 팝업에서 할 수 있다).
  expect(item()!.dataset.state).toBe("notInstalled");
});

it("asks for trust from the untrusted banner", async () => {
  host.trust = "untrusted";
  await mount();
  host.instances.Python = [instance("basedpyright", { kind: "untrusted" })];
  await open(WORDS, 1, 1);
  await settle();
  expect(banner()!.dataset.kind).toBe("untrusted");
  expect(banner()!.textContent).toContain(T.untrusted);
  await click(banner()!.querySelector('[data-action="trust"]'));
  expect(document.querySelector('[data-testid="trust-dialog"]')).toBeTruthy();
});

it("tells about tool updates once a day", async () => {
  host.updates = [
    {
      name: "ruff",
      installed: "0.17.3",
      available: "0.18.0",
      confirmRequired: false,
      checksum: "pinned",
    },
  ];
  await mount();
  await settle();
  expect(called("tools.updates")).toHaveLength(1);
  expect(document.body.textContent).toContain(T.updatesTitle(1));
  expect(document.body.textContent).toContain("ruff 0.17.3 → 0.18.0");
  // 하루 안에 다시 띄운 창은 묻지 않는다.
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  host.calls = [];
  await mount();
  await settle();
  expect(called("tools.updates")).toEqual([]);
});
