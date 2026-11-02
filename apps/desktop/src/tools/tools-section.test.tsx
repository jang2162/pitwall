// @vitest-environment happy-dom
// 설정 창(App kind="settings")의 「도구」(15-m4-plan M4-17): 설치된 도구 표에 판 · 라이선스 · 크기 · 체크섬 유무 · 쓰는 서버가
// 보이고, registry 검색의 설치 · 업데이트는 확인 다이얼로그(처음 포커스 [취소])를 지난 값으로만 다시 부르고, 라이선스가 바뀐
// 업데이트는 다시 묻고, 「최신 registry 사용」 은 `tools.registry.setMode` 로만 쓴다. 가짜 Host 의 설치기 판정은 crates/pw-tools 의
// `covers`(확인 기록이 라이선스 · 출처를 덮는가)와 같다 — 실제 판정 · 받기는 crates/pw-tools/src/install/tests.

import type {
  HostInfo,
  ToolConfirmRequired,
  ToolInstalledInfo,
  ToolPackageSummary,
} from "@pitwall/rpc-client";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import { bundledTranslator } from "../i18n/testing";
import type { KeymapNative } from "../keymap/native";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { act, endTest, it } from "../test-support/ended-test";
import { toolsStrings } from "./strings";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const host = vi.hoisted(() => ({
  calls: [] as { method: string; params: Record<string, unknown> }[],
  installed: [] as ToolInstalledInfo[],
  /** 패키지 → 설치기가 확인을 바라는 값. 없으면 묻지 않는다. */
  consent: {} as Record<string, ToolConfirmRequired | undefined>,
  mode: "pinned" as "pinned" | "latest",
  updates: [] as {
    name: string;
    installed: string;
    available: string;
    confirmRequired: boolean;
    checksum: "pinned" | "none";
  }[],
  opened: [] as string[],
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: async (url: string) => void host.opened.push(url),
}));

function summary(name: string, over: Partial<ToolPackageSummary> = {}): ToolPackageSummary {
  return {
    name,
    description: `${name} <script>alert(1)</script>`,
    version: "2.0.0",
    licenses: ["MIT"],
    languages: ["Python"],
    categories: ["LSP"],
    bundled: true,
    deprecated: false,
    unsupported: null,
    ...over,
  };
}

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
        const m = JSON.parse(text) as {
          id?: number;
          method: string;
          params?: Record<string, unknown>;
        };
        const p = m.params ?? {};
        const reply = (result: unknown) => send({ id: m.id, result });
        if (m.method.startsWith("tools.") || m.method.startsWith("lsp."))
          host.calls.push({ method: m.method, params: p });
        const change = (kind: "install" | "update") => {
          const name = p.name as string;
          const want = host.consent[name];
          const c = p.confirmed as ToolConfirmRequired | null;
          const covered =
            !want ||
            (c !== null &&
              c.version === want.version &&
              JSON.stringify(c.licenses) === JSON.stringify(want.licenses) &&
              JSON.stringify(c.sourceDomains) === JSON.stringify(want.sourceDomains));
          if (!covered)
            return send({
              id: m.id,
              error: {
                code: 4001,
                message: "confirm",
                data: { ...want, rejected: c ? "changed" : null },
              },
            });
          host.consent[name] = undefined;
          return reply({
            name,
            version: kind === "install" ? "2.0.0" : "3.0.0",
            previous: null,
            outcome: "installed",
            checksum: "pinned",
            taskId: 1,
            joined: false,
          });
        };
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
          case "tools.list":
            return reply({ items: host.installed, unreadable: [], installing: [] });
          case "tools.updates":
            return reply({ items: host.updates, registryVersion: "r1" });
          case "tools.registry.status":
            return reply({
              version: host.mode === "latest" ? "r2" : "r1",
              pinnedVersion: "r1",
              mode: host.mode,
              origin: "cache",
              fetchedAtMs: 1_800_000_000_000,
              target: "darwin_arm64",
              supported: 3,
              unsupported: 0,
              unsupportedByReason: [],
              unreadable: 0,
              fetching: false,
              lastError: null,
            });
          case "tools.registry.setMode":
            host.mode = p.mode as "pinned" | "latest";
            return reply({
              version: "r1",
              pinnedVersion: "r1",
              mode: host.mode,
              origin: "cache",
              fetchedAtMs: null,
              target: "darwin_arm64",
              supported: 3,
              unsupported: 0,
              unsupportedByReason: [],
              unreadable: 0,
              fetching: true,
              lastError: null,
            });
          case "tools.registry.search":
            return reply({
              items: [
                summary("basedpyright"),
                summary("ruff", { licenses: ["MIT"], categories: ["Formatter", "Linter"] }),
              ].filter((s) => s.name.includes(String(p.query ?? ""))),
              truncated: false,
            });
          case "tools.registry.get":
            return reply({
              summary: summary(p.name as string),
              homepage: "https://github.com/x/y",
              sourceId: "pkg:pypi/x@2.0.0",
              deprecation: null,
              resolved: null,
              pinned: [],
              checksum: "pinned",
              registryVersion: "r1",
            });
          case "tools.install":
            return change("install");
          case "tools.update":
            return change("update");
          case "tools.log":
            return reply({ text: "pip install …\n<i>log</i>", truncated: false });
          case "lsp.servers.packages":
            return reply([
              { server: "basedpyright", mason: "basedpyright", languages: ["python"] },
              { server: "ruff", mason: "ruff", languages: ["python"] },
            ]);
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

const T = toolsStrings(bundledTranslator("ko"));

const keymapNative: KeymapNative = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
  closeWindow: async () => {},
};

let root: Root | undefined;

function installed(name: string, over: Partial<ToolInstalledInfo> = {}): ToolInstalledInfo {
  return {
    name,
    version: "1.0.0",
    previous: null,
    kind: "pypi",
    licenses: ["MIT"],
    checksum: "pinned",
    size: 14_365_000,
    installedAtMs: 1,
    sourceUrls: [],
    bins: [name],
    problem: null,
    ...over,
  };
}

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(console, "info").mockImplementation(() => {});
  host.calls = [];
  host.installed = [];
  host.consent = {};
  host.mode = "pinned";
  host.updates = [];
  host.opened = [];
  localStorage.clear();
});

afterEach(async () => {
  await endTest();
  await act(async () => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 200)));
const section = () => document.querySelector<HTMLElement>('[data-testid="tools-section"]')!;
const dialog = () => document.querySelector<HTMLElement>('[data-testid="tool-confirm-dialog"]');
const called = (method: string) =>
  host.calls.filter((c) => c.method === method).map((c) => c.params);

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
  await vi.waitFor(() => expect(section()?.textContent).toContain(T.manage.title));
  await settle();
}

async function click(el: Element | null | undefined) {
  expect(el).toBeTruthy();
  await act(async () => (el as HTMLElement).click());
  await settle();
}

const resultRow = (name: string) =>
  section().querySelector<HTMLElement>(`[data-testid="tools-result-row"][data-name="${name}"]`)!;

it("lists installed tools with version, license, size, checksum and the servers that use them", async () => {
  host.installed = [
    installed("basedpyright", { previous: "0.9.0" }),
    installed("ruff", { checksum: "none", size: 512, licenses: ["MIT", "LicenseRef-x"] }),
  ];
  await mount();
  const row = (name: string) =>
    section().querySelector<HTMLElement>(`[data-testid="tools-row"][data-name="${name}"]`)!;
  expect(row("basedpyright").textContent).toContain("1.0.0");
  expect(row("basedpyright").textContent).toContain(T.manage.previous("0.9.0"));
  expect(row("basedpyright").querySelector('[data-testid="tools-size"]')!.textContent).toBe(
    "14 MiB",
  );
  expect(row("basedpyright").querySelector('[data-testid="tools-license"]')!.textContent).toBe(
    "MIT",
  );
  expect(row("basedpyright").querySelector('[data-testid="tools-checksum"]')!.textContent).toBe(
    T.manage.checksumPinned,
  );
  expect(row("basedpyright").textContent).toContain("Python");
  expect(row("basedpyright").textContent).toContain("LSP");
  expect(row("basedpyright").textContent).toContain("basedpyright");
  expect(row("ruff").querySelector('[data-testid="tools-size"]')!.textContent).toBe("512 B");
  expect(row("ruff").querySelector('[data-testid="tools-checksum"]')!.textContent).toBe(
    T.manage.checksumNone,
  );
  expect(row("ruff").querySelector('[data-testid="tools-license"]')!.textContent).toBe(
    "MIT · LicenseRef-x",
  );
  // registry 의 글은 글로만(검색 결과의 설명).
  expect(section().querySelector("script")).toBeNull();
});

it("installs from the registry only through the dialog and calls nothing more when cancelled", async () => {
  host.consent.basedpyright = {
    name: "basedpyright",
    version: "2.0.0",
    licenses: ["MIT"],
    sourceDomains: ["pypi.org"],
    size: null,
    checksum: "none",
    note: null,
    rejected: null,
  };
  await mount();
  await click(resultRow("basedpyright").querySelector('[data-testid="tools-install"]'));
  expect(dialog()).toBeTruthy();
  expect(document.activeElement?.getAttribute("data-testid")).toBe("tool-confirm-cancel");
  expect(
    dialog()!.querySelector<HTMLElement>('[data-testid="tool-confirm-integrity"]')!.dataset
      .checksum,
  ).toBe("none");
  // 홈페이지는 http(s) 라 링크다 — 누르면 외부로 연다.
  await click(dialog()!.querySelector('a[href="https://github.com/x/y"]'));
  expect(host.opened).toEqual(["https://github.com/x/y"]);
  await click(dialog()!.querySelector('[data-testid="tool-confirm-cancel"]'));
  expect(called("tools.install")).toEqual([
    { name: "basedpyright", version: null, confirmed: null },
  ]);

  await click(resultRow("basedpyright").querySelector('[data-testid="tools-install"]'));
  await click(dialog()!.querySelector('[data-testid="tool-confirm-accept"]'));
  expect(called("tools.install")).toHaveLength(3);
  expect(called("tools.install")[2]).toEqual({
    name: "basedpyright",
    version: null,
    confirmed: { version: "2.0.0", licenses: ["MIT"], sourceDomains: ["pypi.org"] },
  });
  expect(section().querySelector('[data-testid="tools-result"]')!.textContent).toBe(
    T.manage.done("basedpyright", "2.0.0"),
  );
});

it("asks again before an update whose license changed", async () => {
  host.installed = [installed("ruff")];
  host.updates = [
    {
      name: "ruff",
      installed: "1.0.0",
      available: "3.0.0",
      confirmRequired: true,
      checksum: "pinned",
    },
  ];
  host.consent.ruff = {
    name: "ruff",
    version: "3.0.0",
    licenses: ["GPL-3.0-only"],
    sourceDomains: ["github.com"],
    size: 4096,
    checksum: "pinned",
    note: "the license changed",
    rejected: null,
  };
  await mount();
  await click(section().querySelector('[data-testid="tools-update"]'));
  expect(dialog()!.dataset.kind).toBe("update");
  expect(dialog()!.textContent).toContain("GPL-3.0-only");
  expect(dialog()!.querySelector('[data-testid="tool-confirm-note"]')!.textContent).toBe(
    "the license changed",
  );
  expect(document.activeElement?.getAttribute("data-testid")).toBe("tool-confirm-cancel");
  expect(called("tools.update")).toEqual([{ name: "ruff", confirmed: null }]);
  await click(dialog()!.querySelector('[data-testid="tool-confirm-accept"]'));
  expect(called("tools.update")).toHaveLength(2);
});

it("updates without asking when the installer has a covering confirmation", async () => {
  host.installed = [installed("ruff")];
  host.updates = [
    {
      name: "ruff",
      installed: "1.0.0",
      available: "3.0.0",
      confirmRequired: false,
      checksum: "pinned",
    },
  ];
  await mount();
  await click(section().querySelector('[data-testid="tools-update"]'));
  expect(dialog()).toBeNull();
  expect(called("tools.update")).toEqual([{ name: "ruff", confirmed: null }]);
});

it("writes the latest registry setting through tools.registry.setMode only", async () => {
  await mount();
  const toggle = section().querySelector<HTMLInputElement>(
    '[data-testid="tools-registry-latest"]',
  )!;
  expect(toggle.checked).toBe(false);
  expect(section().querySelector('[data-testid="tools-registry"]')!.textContent).toContain(
    T.manage.registryVersion("r1", "r1"),
  );
  await click(toggle);
  expect(called("tools.registry.setMode")).toEqual([{ mode: "latest" }]);
  expect(
    section().querySelector<HTMLInputElement>('[data-testid="tools-registry-latest"]')!.checked,
  ).toBe(true);
  await click(section().querySelector('[data-testid="tools-registry-latest"]'));
  expect(called("tools.registry.setMode")).toEqual([{ mode: "latest" }, { mode: "pinned" }]);
});
