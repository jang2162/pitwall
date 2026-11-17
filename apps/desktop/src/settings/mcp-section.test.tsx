// @vitest-environment happy-dom
// 설정 창(App kind="settings")의 「MCP」(16-m5-plan M5-15): 그룹 · 툴 토글과 프리셋 · 자동 승인 · 보관 일수가 `mcp.settings.set` 으로
// 가고(전역만 — 판정은 Host), 검색이 목록을 거르고, 고른 프로젝트가 판정을 바꾼 툴에 표지를 달고 토글을 잠그며, 등록 문구를
// 복사한다. 가짜 Host 는 판정을 흉내 내지 않는다 — 돌려줄 값을 시험이 정한다(Host 쪽 판정 · 파일 쓰기는
// crates/pw-mcp/tests/settings.rs).

import type { HostInfo, McpSettingsView, McpToolInfo, Project } from "@pitwall/rpc-client";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import { bundledTranslator } from "../i18n/testing";
import type { KeymapNative } from "../keymap/native";
import { mcpStrings } from "../mcp/strings";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { act, endTest, it } from "../test-support/ended-test";
import { REGISTRATIONS } from "./mcp-section";
import type { SettingsNative } from "./native";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const host = vi.hoisted(() => ({
  view: null as unknown as import("@pitwall/rpc-client").McpSettingsView,
  projectView: null as unknown as import("@pitwall/rpc-client").McpSettingsView["project"],
  tools: [] as import("@pitwall/rpc-client").McpToolInfo[],
  projectTools: [] as import("@pitwall/rpc-client").McpToolInfo[],
  /** 다음 `mcp.settings.set` 을 이 까닭으로 거절한다. */
  refuse: undefined as string | undefined,
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
          case "project.list":
            return reply([
              { id: "p1", name: "alpha" },
              { id: "p2", name: "beta" },
            ] as Project[]);
          case "mcp.settings.get": {
            const p = m.params as { projectId?: string | null };
            return reply({ ...host.view, project: p.projectId ? host.projectView : null });
          }
          case "mcp.tools.list": {
            const p = m.params as { projectId?: string | null };
            return reply(p.projectId ? host.projectTools : host.tools);
          }
          case "mcp.settings.set": {
            const p = m.params as { key: string; name: string | null; value: unknown };
            if (host.refuse) {
              const reason = host.refuse;
              host.refuse = undefined;
              return send({
                id: m.id,
                error: { code: -32602, message: "refused", data: { key: p.key, reason } },
              });
            }
            // 가짜 Host: 받은 값을 그대로 돌려줄 상태에 둔다(판정은 Rust 시험이 본다).
            if (p.key === "group")
              host.view.groups = host.view.groups.map((g) =>
                g.group === p.name ? { ...g, enabled: p.value as boolean | null } : g,
              );
            if (p.key === "tool")
              host.tools = host.tools.map((t) =>
                t.name === p.name
                  ? p.value === null
                    ? { ...t, enabled: true, rule: "preset", key: "standard" }
                    : { ...t, enabled: p.value as boolean, rule: "tool", key: t.name }
                  : t,
              );
            if (p.key === "preset") host.view.preset = p.value as string;
            if (p.key === "autoApprove")
              host.view.autoApprove = p.value as McpSettingsView["autoApprove"];
            if (p.key === "retentionDays")
              host.view.retentionDays = p.value === null ? 90 : (p.value as number);
            return reply({ ...host.view, project: null });
          }
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

const S = mcpStrings(bundledTranslator("ko")).settings;

function tool(name: string, group: string, over: Partial<McpToolInfo> = {}): McpToolInfo {
  return {
    name,
    description: `Does ${name}.`,
    tier: "read",
    group,
    inputSchema: { type: "object", properties: { projectId: { type: "string" } } },
    enabled: true,
    rule: "preset",
    key: "standard",
    recentCalls: 0,
    ...over,
  };
}

function view(): McpSettingsView {
  return {
    file: "/home/me/.config/pitwall/settings.toml",
    preset: "standard",
    autoApprove: ["read"],
    groups: ["project", "run", "debug", "db", "git", "code", "open_with"].map((group) => ({
      group,
      enabled: null,
    })),
    retentionDays: 90,
    defaultRetentionDays: 90,
    warnings: [],
    project: null,
  };
}

const keymapNative: KeymapNative = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
  closeWindow: async () => {},
};

let root: Root | undefined;
let native: SettingsNative & { copied: string[]; opened: string[] };

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(console, "info").mockImplementation(() => {});
  host.calls = [];
  host.view = view();
  host.refuse = undefined;
  host.tools = [
    tool("project_list", "project", { recentCalls: 4 }),
    tool("db_query", "db"),
    tool("db_execute", "db", { tier: "dangerous", enabled: false, key: "standard" }),
    tool("git_status", "git"),
  ];
  host.projectTools = host.tools.map((t) =>
    t.name === "db_query" ? { ...t, enabled: false, rule: "group", key: "db" } : t,
  );
  host.projectView = {
    projectId: "p1",
    files: [
      { kind: "shared", path: "/work/alpha/.pitwall/project.toml", exists: true },
      { kind: "local", path: "/work/alpha/.pitwall/project.local.toml", exists: false },
    ],
    overrides: [{ key: "groups.db", value: false, file: "shared" }],
    autoApprove: [],
    warnings: [],
  };
  native = {
    copied: [],
    opened: [],
    copyText: async (text) => {
      native.copied.push(text);
    },
    openFile: async (path) => {
      native.opened.push(path);
    },
  };
  localStorage.clear();
});

afterEach(async () => {
  await endTest();
  await act(async () => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 20)));
const section = () => document.querySelector('section[aria-labelledby="settings-mcp"]')!;
const part = (name: string) => section().querySelector<HTMLElement>(`[data-setting="${name}"]`)!;
const row = (name: string) => section().querySelector<HTMLElement>(`tr[data-tool="${name}"]`);
const sets = () => host.calls.filter((c) => c.method === "mcp.settings.set").map((c) => c.params);

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
        <App kind="settings" native={keymapNative} settingsNative={native} />
      </ThemeStoreProvider>,
    ),
  );
  await vi.waitFor(() => expect(row("db_query")).toBeTruthy());
  await vi.waitFor(() => expect(part("groups")).toBeTruthy());
}

async function choose(select: HTMLSelectElement, value: string) {
  await act(async () => {
    // happy-dom 은 `value = ""` 로 빈 값의 option 을 고르지 않는다 — 차례로 고른다.
    select.selectedIndex = [...select.options].findIndex((o) => o.value === value);
    select.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await settle();
}

async function click(element: HTMLElement) {
  await act(async () => element.click());
  await settle();
}

async function type(input: HTMLInputElement, text: string) {
  await act(async () => {
    const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    set.call(input, text);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();
}

const groupSelect = (group: string) =>
  section().querySelector<HTMLSelectElement>(`#mcp-group-${group}`)!;
const toolBox = (name: string) => row(name)!.querySelector<HTMLInputElement>("input")!;

it("a group toggle writes [mcp.groups] through the host and shows the value read back", async () => {
  await mount();
  expect(groupSelect("db").value).toBe("default");
  await choose(groupSelect("db"), "off");
  expect(sets()).toEqual([{ key: "group", name: "db", value: false }]);
  await vi.waitFor(() => expect(groupSelect("db").value).toBe("off"));
  // 「프리셋을 따름」 은 그 키를 지운다.
  await choose(groupSelect("db"), "default");
  expect(sets()[1]).toEqual({ key: "group", name: "db", value: null });
  await vi.waitFor(() => expect(groupSelect("db").value).toBe("default"));
});

it("a tool toggle writes [mcp.tools] and its default button removes the key", async () => {
  await mount();
  expect(toolBox("db_query").checked).toBe(true);
  expect(row("db_query")!.querySelector("[data-why]")!.textContent).toBe(S.rule.preset("standard"));
  await click(toolBox("db_query"));
  expect(sets()).toEqual([{ key: "tool", name: "db_query", value: false }]);
  await vi.waitFor(() => expect(toolBox("db_query").checked).toBe(false));
  expect(row("db_query")!.querySelector("[data-why]")!.textContent).toBe(S.rule.tool);
  const reset = [...row("db_query")!.querySelectorAll("button")].find(
    (b) => b.textContent === S.reset,
  )!;
  await click(reset);
  expect(sets()[1]).toEqual({ key: "tool", name: "db_query", value: null });
  await vi.waitFor(() => expect(toolBox("db_query").checked).toBe(true));
});

it("shows the host's verdict, tier, group, recent calls and the input schema", async () => {
  await mount();
  const off = row("db_execute")!;
  expect(toolBox("db_execute").checked).toBe(false);
  expect(off.textContent).toContain("Does db_execute.");
  expect(off.textContent).toContain(mcpStrings(bundledTranslator("ko")).approval.tier.dangerous);
  expect(row("project_list")!.textContent).toContain("4");
  expect(section().querySelector('[data-schema="db_query"]')).toBeNull();
  await click(
    [...row("db_query")!.querySelectorAll("button")].find((b) => b.textContent === S.schemaShow)!,
  );
  expect(section().querySelector('[data-schema="db_query"]')!.textContent).toContain('"projectId"');
});

it("search narrows the list by name, group or description", async () => {
  await mount();
  const search = part("tools").querySelector<HTMLInputElement>('input[type="search"]')!;
  await type(search, "db");
  expect(row("db_query")).toBeTruthy();
  expect(row("db_execute")).toBeTruthy();
  expect(row("project_list")).toBeNull();
  expect(row("git_status")).toBeNull();
  await type(search, "does git");
  expect(row("git_status")).toBeTruthy();
  expect(row("db_query")).toBeNull();
  await type(search, "nothing-like-this");
  expect(part("tools").textContent).toContain(S.noTools);
});

it("marks and locks the tools a chosen project overrides, and opens only the host's files", async () => {
  await mount();
  const picker = part("project").querySelector("select")!;
  await choose(picker, "p1");
  await vi.waitFor(() => expect(row("db_query")!.dataset.overridden).toBe(""));
  const overridden = row("db_query")!;
  expect(toolBox("db_query").disabled).toBe(true);
  expect(overridden.querySelector("[data-project-verdict]")!.textContent).toContain(
    S.overridden(S.stateOff, S.rule.group("db")),
  );
  // 판정이 같은 툴은 표지가 없고 전역 토글 그대로다.
  expect(row("git_status")!.dataset.overridden).toBeUndefined();
  expect(toolBox("git_status").disabled).toBe(false);
  // 그 프로젝트가 적은 값과 파일.
  const overrides = part("project").querySelector("[data-project-overrides]")!;
  expect(overrides.querySelector('[data-override-key="groups.db"]')!.textContent).toContain(
    "false",
  );
  expect(groupSelect("db").closest("[data-group]")!.textContent).toContain(S.overriddenBadge);
  const shared = overrides.querySelector('[data-file="shared"]')!;
  await click(shared.querySelector("button")!);
  expect(native.opened).toEqual(["/work/alpha/.pitwall/project.toml"]);
  // 없는 파일은 열 단추가 없다.
  expect(overrides.querySelector('[data-file="local"] button')).toBeNull();
  // 전역 파일도 Host 가 준 경로로 연다.
  await click([...section().querySelectorAll("button")].find((b) => b.textContent === S.openFile)!);
  expect(native.opened[1]).toBe("/home/me/.config/pitwall/settings.toml");
  // 프로젝트를 고르는 것은 아무것도 쓰지 않는다.
  expect(sets()).toEqual([]);
  // 없음으로 돌리면 표지가 사라진다.
  await choose(picker, "");
  await vi.waitFor(() => expect(row("db_query")!.dataset.overridden).toBeUndefined());
  expect(toolBox("db_query").disabled).toBe(false);
});

it("the retention field shows the value and edits it, and shows why the host refused", async () => {
  host.view.retentionDays = 14;
  await mount();
  const input = part("retentionDays").querySelector("input")!;
  expect(input.value).toBe("14");
  await type(input, "30");
  await act(async () => part("retentionDays").querySelector("form")!.requestSubmit());
  await settle();
  expect(sets()).toEqual([{ key: "retentionDays", name: null, value: 30 }]);
  await vi.waitFor(() => expect(input.value).toBe("30"));

  host.refuse = "badRetentionDays";
  await type(input, "-1");
  await act(async () => part("retentionDays").querySelector("form")!.requestSubmit());
  await settle();
  expect(sets()[1]).toEqual({ key: "retentionDays", name: null, value: -1 });
  expect(part("retentionDays").querySelector("[role=alert]")!.textContent).toBe(
    S.refused.badRetentionDays,
  );

  await click(
    [...part("retentionDays").querySelectorAll("button")].find((b) => b.textContent === S.reset)!,
  );
  expect(sets()[2]).toEqual({ key: "retentionDays", name: null, value: null });
});

it("preset and auto-approve go to the host, and there is no dangerous auto-approve box", async () => {
  await mount();
  await choose(part("preset").querySelector("select")!, "full");
  expect(sets()).toEqual([{ key: "preset", name: null, value: "full" }]);
  const boxes = [...part("autoApprove").querySelectorAll<HTMLInputElement>("input[data-tier]")];
  expect(boxes.map((b) => b.dataset.tier)).toEqual(["read", "write", "exec"]);
  await click(boxes[1]!);
  expect(sets()[1]).toEqual({ key: "autoApprove", name: null, value: ["read", "write"] });
  host.refuse = "dangerousAutoApprove";
  await click(boxes[2]!);
  expect(part("autoApprove").querySelector("[role=alert]")!.textContent).toBe(
    S.refused.dangerousAutoApprove,
  );
});

it("copies each agent's registration text", async () => {
  await mount();
  for (const r of REGISTRATIONS) {
    const line = part("register").querySelector<HTMLElement>(`[data-register="${r.id}"]`)!;
    expect(line.querySelector("code")!.textContent).toBe(r.text);
    await click(line.querySelector("button")!);
    expect(line.querySelector("[role=status]")!.textContent).toBe(S.copied);
  }
  expect(native.copied).toEqual(REGISTRATIONS.map((r) => r.text));
  expect(native.copied[0]).toBe("claude mcp add --scope user pitwall -- pitwall mcp");
});
