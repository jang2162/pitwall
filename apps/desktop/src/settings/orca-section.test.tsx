// @vitest-environment happy-dom
// 설정 창(App kind="settings")을 가짜 Host 에 붙여 「Orca 에서 Pitwall 열기」 를 본다: 등록 값의 복사 버튼, `cli.status` 의 판정별
// 표시와 「Shell 명령 설치」 · 절대 경로 안내, 설치 뒤 다시 검사하면 바뀐 판정이 보이는 것, 로그인 셸이 백그라운드로 살아나면
// (Host 범위 `runtime.changed`) 누르지 않아도 다시 검사하는 것.

import type { CliStatus, HostInfo } from "@pitwall/rpc-client";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import { bundledTranslator } from "../i18n/testing";
import type { KeymapNative } from "../keymap/native";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import type { SettingsNative } from "./native";
import { settingsStrings } from "./strings";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const TARGET = "/tmp/pw-test/bin/pitwall";
const APP = "/Applications/Pitwall.app/Contents/MacOS/pitwall";

const host = vi.hoisted(() => ({
  /** `cli.status` 의 응답. `null` 이면 오류로 답한다. */
  status: null as unknown,
  statusCalls: 0,
  /** Host 범위로 구독한 창에 이벤트를 보낸다. 연결마다 바뀐다. */
  emit: (_topic: string, _payload: unknown) => {},
}));

vi.mock("../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../keymap/default-keymap");
  const { i18nState } = await import("../i18n/testing");
  return {
    connectHost: async () => {
      const [client, server] = pair();
      const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
      const subs = new Map<number, { projectId?: string; topics: string[] }>();
      let next = 1;
      host.emit = (topic, payload) => {
        for (const [subscriptionId, s] of subs) {
          if (s.projectId !== undefined || !s.topics.includes(topic)) continue;
          send({
            method: "events.event",
            params: { subscriptionId, projectId: null, topic, payload },
          });
        }
      };
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params?: unknown };
        const reply = (result: unknown) => send({ id: m.id, result });
        switch (m.method) {
          case "i18n.get":
            return reply(i18nState("ko"));
          case "config.get":
            return reply({ snapshot: { files: [], notices: [] }, error: null });
          case "keymap.get":
            return reply({
              id: "pitwall-default",
              extension: "dev.pitwall.keymap-default",
              kind: "base",
              content: DEFAULT_KEYMAP_TOML,
            });
          case "host.info":
            return reply({
              hostId: "local",
              kind: "local",
              version: "0",
              os: "t",
              arch: "t",
            } satisfies HostInfo);
          case "cli.status":
            host.statusCalls++;
            if (host.status === null) {
              return send({
                id: m.id,
                error: { code: -32603, message: "internal error", data: "cannot locate" },
              });
            }
            return reply(host.status);
          case "events.subscribe": {
            const id = next++;
            subs.set(id, m.params as { projectId?: string; topics: string[] });
            return reply({ subscriptionId: id });
          }
          case "events.unsubscribe":
            return reply(null);
        }
      });
      return { transport: client, close: async () => {} };
    },
  };
});

const T = settingsStrings(bundledTranslator("ko")).orca;

function status(
  install: CliStatus["install"],
  onPath: CliStatus["onPath"],
  pathStatus: CliStatus["pathStatus"] = { kind: "ok" },
  source: CliStatus["source"] = { kind: "ok" },
): CliStatus {
  return {
    target: TARGET,
    app: APP,
    source,
    install,
    onPath,
    pathSource: { kind: "loginShell", shell: "/bin/zsh" },
    pathStatus,
  };
}

const MISSING = status({ kind: "missing" }, { kind: "notFound" });
const INSTALLED = status({ kind: "thisApp" }, { kind: "thisApp", path: TARGET });

const keymapNative: KeymapNative = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
  closeWindow: async () => {},
};

let root: Root | undefined;
let native: {
  copyText: ReturnType<typeof vi.fn<SettingsNative["copyText"]>>;
  installShellCommand?: ReturnType<
    typeof vi.fn<NonNullable<SettingsNative["installShellCommand"]>>
  >;
};

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(console, "info").mockImplementation(() => {});
  host.status = MISSING;
  host.statusCalls = 0;
  native = {
    copyText: vi.fn(async () => {}),
    installShellCommand: vi.fn(async () => {
      host.status = INSTALLED;
    }),
  };
  localStorage.clear();
});

afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 20)));

async function mount() {
  act(() => root?.unmount());
  document.body.innerHTML = "";
  const element = document.createElement("div");
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
        <App kind="settings" native={keymapNative} settingsNative={native} />
      </ThemeStoreProvider>,
    ),
  );
  await settle();
}

const headline = () => document.querySelector('[data-testid="orca-headline"]')?.textContent;
const section = () => document.querySelector('section[aria-labelledby="settings-orca"]')!;
const button = (name: string) =>
  [...section().querySelectorAll("button")].find(
    (b) => b.textContent === name || b.getAttribute("aria-label") === name,
  );

async function click(element: Element | undefined) {
  expect(element).toBeDefined();
  await act(async () => (element as HTMLElement).click());
  await settle();
}

it("shows Not installed with the install button, and Installed · On PATH after installing and reopening", async () => {
  await mount();
  expect(headline()).toBe(`${T.install.missing} · ${T.onPath.notFound}`);
  expect(section().textContent).toContain(T.detail.missing(TARGET));
  const install = button(T.installButton);
  expect(install).toBeDefined();

  await click(install);
  expect(native.installShellCommand).toHaveBeenCalledTimes(1);
  // 설치가 끝나면 다시 검사한다.
  expect(headline()).toBe(`${T.install.thisApp} · ${T.onPath.thisApp}`);
  expect(button(T.installButton)).toBeUndefined();

  // 창을 다시 열면 새로 검사한다.
  const calls = host.statusCalls;
  await mount();
  expect(host.statusCalls).toBe(calls + 1);
  expect(headline()).toBe(`${T.install.thisApp} · ${T.onPath.thisApp}`);
  expect(section().textContent).toContain(T.detail.installed(TARGET));
});

it("checks again after a cancelled or failed install", async () => {
  native.installShellCommand!.mockRejectedValueOnce(new Error("cancelled"));
  await mount();
  const calls = host.statusCalls;
  await click(button(T.installButton));
  expect(host.statusCalls).toBe(calls + 1);
  expect(headline()).toBe(`${T.install.missing} · ${T.onPath.notFound}`);
});

it("shows where a link to somewhere else points, and offers to install over it", async () => {
  host.status = status(
    { kind: "otherLink", target: "/Volumes/Pitwall/Pitwall.app/Contents/MacOS/pitwall" },
    {
      kind: "other",
      path: TARGET,
      resolved: "/Volumes/Pitwall/Pitwall.app/Contents/MacOS/pitwall",
    },
  );
  await mount();
  expect(headline()).toBe(`${T.install.otherLink} · ${T.onPath.other}`);
  const text = section().textContent;
  expect(text).toContain(
    T.detail.otherLink(TARGET, "/Volumes/Pitwall/Pitwall.app/Contents/MacOS/pitwall"),
  );
  expect(text).toContain(T.detail.otherOnPath(TARGET));
  expect(text).toContain(T.detail.resolved("/Volumes/Pitwall/Pitwall.app/Contents/MacOS/pitwall"));
  expect(button(T.installButton)).toBeDefined();
});

it("copies the label and the command", async () => {
  await mount();
  await click(button(T.copyValue("pitwall")));
  expect(native.copyText).toHaveBeenLastCalledWith("pitwall");
  expect(section().textContent).toContain(T.copied);
  await click(button(T.copyValue("Pitwall")));
  expect(native.copyText).toHaveBeenLastCalledWith("Pitwall");
});

it("says so when the clipboard cannot be written", async () => {
  native.copyText.mockRejectedValueOnce({ message: "no clipboard" });
  await mount();
  await click(button(T.copyValue("pitwall")));
  expect(section().querySelector('[role="alert"]')?.textContent).toBe(T.copyFailed("no clipboard"));
});

it("guides to register the absolute path when the installed command is not on PATH", async () => {
  host.status = status({ kind: "thisApp" }, { kind: "notFound" });
  await mount();
  expect(headline()).toBe(`${T.install.thisApp} · ${T.onPath.notFound}`);
  expect(section().textContent).toContain(T.absolute(TARGET));
  expect(button(T.installButton)).toBeUndefined();
  await click(button(T.copyValue(TARGET)));
  expect(native.copyText).toHaveBeenLastCalledWith(TARGET);
});

it("guides to the absolute path when another pitwall shadows the installed one", async () => {
  host.status = status(
    { kind: "thisApp" },
    { kind: "other", path: "/opt/homebrew/bin/pitwall", resolved: null },
  );
  await mount();
  expect(section().textContent).toContain(T.detail.otherOnPath("/opt/homebrew/bin/pitwall"));
  expect(section().textContent).not.toContain(T.detail.resolved(""));
  expect(section().textContent).toContain(T.absolute(TARGET));
});

it("does not offer to install over a file that is not a link", async () => {
  host.status = status({ kind: "notALink" }, { kind: "notFound" });
  await mount();
  expect(headline()).toBe(`${T.install.notALink} · ${T.onPath.notFound}`);
  expect(section().textContent).toContain(T.detail.notALink(TARGET));
  expect(button(T.installButton)).toBeUndefined();
});

it("has no install button where the shell command cannot be installed", async () => {
  delete native.installShellCommand;
  await mount();
  expect(headline()).toBe(`${T.install.missing} · ${T.onPath.notFound}`);
  expect(button(T.installButton)).toBeUndefined();
});

it("does not offer to install when the pitwall on PATH is already this app", async () => {
  host.status = status(
    { kind: "missing" },
    { kind: "thisApp", path: "/Users/me/.local/bin/pitwall" },
  );
  await mount();
  expect(headline()).toBe(T.worksFromPath);
  expect(section().textContent).toContain(
    T.detail.worksFromPath("/Users/me/.local/bin/pitwall", TARGET),
  );
  expect(section().textContent).not.toContain(T.detail.missing(TARGET));
  expect(button(T.installButton)).toBeUndefined();
  expect(section().textContent).not.toContain(T.absolute(TARGET));
});

it("does not offer to install a development build over the real command, and says why", async () => {
  const DEV = "/Users/me/code/pitwall/target/debug/pitwall";
  host.status = {
    ...status(
      { kind: "otherLink", target: APP },
      { kind: "other", path: TARGET, resolved: APP },
      undefined,
      {
        kind: "developmentBuild",
      },
    ),
    app: DEV,
  };
  await mount();
  expect(headline()).toBe(`${T.install.otherLink} · ${T.onPath.other}`);
  expect(button(T.installButton)).toBeUndefined();
  expect(section().textContent).toContain(T.source.developmentBuild(DEV, TARGET));

  host.status = status({ kind: "missing" }, { kind: "notFound" }, undefined, { kind: "transient" });
  await mount();
  expect(button(T.installButton)).toBeUndefined();
  expect(section().textContent).toContain(T.source.transient(APP));
  expect(native.installShellCommand).not.toHaveBeenCalled();
});

it("notes when PATH came from the app instead of the login shell", async () => {
  host.status = status(
    { kind: "missing" },
    { kind: "notFound" },
    {
      kind: "fallback",
      reason: "the shell exited with 7",
      problem: "loginShell",
    },
  );
  await mount();
  expect(section().textContent).toContain(T.pathFallback("the shell exited with 7"));
});

it("shows a failed check and the .worktreeinclude guide", async () => {
  host.status = null;
  await mount();
  expect(section().textContent).toContain(T.checkFailed("internal error (-32603): cannot locate"));
  expect(section().textContent).toContain(T.worktree);
  expect(T.worktree).toContain(".worktreeinclude");
});

it("checks again by itself when the login shell recovers in the background", async () => {
  // 시간을 넘긴 로그인 셸: 앱 프로세스의 PATH 로 물러나 이 앱을 찾지 못한다.
  host.status = status(
    { kind: "thisApp" },
    { kind: "notFound" },
    { kind: "fallback", reason: "timed out", problem: "loginShell" },
  );
  await mount();
  expect(section().textContent).toContain(T.pathFallback("timed out"));
  const calls = host.statusCalls;

  // 백그라운드 재시도가 셸 환경을 얻으면 Host 가 Host 범위에 `runtime.changed`(loginShell)를 낸다 — 「다시 검사」 없이 바뀐다.
  host.status = INSTALLED;
  await act(async () => host.emit("runtime.changed", { reason: "loginShell" }));
  await settle();
  expect(host.statusCalls).toBe(calls + 1);
  expect(headline()).toBe(`${T.install.thisApp} · ${T.onPath.thisApp}`);
  expect(section().textContent).not.toContain(T.pathFallback("timed out"));
});
