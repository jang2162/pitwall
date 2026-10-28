// @vitest-environment happy-dom
// 설정 창(App kind="settings")의 「Git」(13-m2-plan.md M2-20 완료 기준): `git.executable` 이 준 버전이 이 기기의 `git --version` 과 같게
// 보인다(Host 쪽 — RPC 가 `git --version` 과 같은 버전을 내는 것 — 은 crates/pw-git/src/executable/tests.rs), 쓰기를 끈 이유가
// 갈래마다 보인다, 로그인 셸을 읽지 못했으면 그렇게 적는다. 설정 창은 Host 연결로 읽기만 한다 — git 쓰기 RPC 를 부르지 않는다.

import type { GitExecutable, HostInfo } from "@pitwall/rpc-client";
import { execFileSync } from "node:child_process";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import { gitDialogStrings } from "../git/dialogs/strings";
import { bundledTranslator } from "../i18n/testing";
import type { KeymapNative } from "../keymap/native";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const host = vi.hoisted(() => ({
  info: null as unknown,
  calls: [] as { method: string; params: unknown }[],
}));

vi.mock("../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../keymap/default-keymap");
  const { i18nState } = await import("../i18n/testing");
  return {
    connectHost: async () => {
      const [client, server] = pair();
      const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
      let next = 1;
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params?: unknown };
        const reply = (result: unknown) => send({ id: m.id, result });
        host.calls.push({ method: m.method, params: m.params });
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
          case "git.executable":
            return reply(host.info);
          case "events.subscribe":
            return reply({ subscriptionId: next++ });
          case "events.unsubscribe":
            return reply(null);
        }
      });
      return { transport: client, close: async () => {} };
    },
  };
});

const T = gitDialogStrings(bundledTranslator("ko")).settings;

/** 이 기기의 `git --version`(Host 의 `GitVersion::parse` 와 같은 규칙 — 첫 줄의 `git version` 뒤 숫자 셋, 패치가 없으면 0). */
function machineGitVersion(): string {
  const line = execFileSync("git", ["--version"], { encoding: "utf8" }).split("\n")[0]!;
  const parts = line.replace(/^git version /, "").match(/^(\d+)\.(\d+)(?:\.(\d+))?/)!;
  return `${parts[1]}.${parts[2]}.${parts[3] ?? "0"}`;
}

function info(over: Partial<GitExecutable> = {}): GitExecutable {
  return {
    configured: null,
    program: "/usr/bin/git",
    version: machineGitVersion(),
    minimum: "2.40",
    writesOff: null,
    pathSource: { kind: "loginShell", shell: "/bin/zsh" },
    shellFailure: null,
    ...over,
  };
}

const keymapNative: KeymapNative = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
  closeWindow: async () => {},
};

let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(console, "info").mockImplementation(() => {});
  host.calls = [];
  host.info = info();
  localStorage.clear();
});

afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 20)));
const text = (id: string) => document.querySelector(`[data-testid=${id}]`)?.textContent;
const section = () => document.querySelector('section[aria-labelledby="settings-git"]')!;

async function mount() {
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
        <App kind="settings" native={keymapNative} settingsNative={{ copyText: async () => {} }} />
      </ThemeStoreProvider>,
    ),
  );
  await vi.waitFor(() => expect(text("settings-git-version")).toBeDefined());
}

it("shows the same version as this machine's git --version, the path and that writes are on", async () => {
  await mount();
  expect(text("settings-git-version")).toBe(machineGitVersion());
  expect(text("settings-git-program")).toBe("/usr/bin/git");
  expect(text("settings-git-writes")).toBe(T.writesOn);
  expect(section().textContent).toContain(T.notConfigured);
  expect(section().textContent).toContain(T.shellPath);
  // 읽기만 한다: git 은 `git.executable` 하나뿐이다.
  expect(host.calls.filter((c) => c.method.startsWith("git.")).map((c) => c.method)).toEqual([
    "git.executable",
  ]);
});

it("says why writes are off", async () => {
  host.info = info({
    version: "2.39.5",
    writesOff: { kind: "tooOld", version: "2.39.5" },
    configured: "/opt/git/bin/git",
    program: "/opt/git/bin/git",
  });
  await mount();
  expect(text("settings-git-writes")).toBe(T.tooOld("2.39.5", "2.40"));
  expect(section().textContent).toContain(T.configured("/opt/git/bin/git"));

  act(() => root?.unmount());
  document.body.innerHTML = "";
  host.info = info({
    program: null,
    version: null,
    writesOff: { kind: "notFound", program: "git" },
    pathSource: { kind: "process" },
    shellFailure: "timed out",
  });
  await mount();
  await settle();
  expect(text("settings-git-version")).toBe("—");
  expect(text("settings-git-writes")).toBe(T.notFound("git"));
  expect(section().textContent).toContain(T.processPath("timed out"));
});
