// @vitest-environment happy-dom
// 설정 창(App kind="settings")의 「DB」(14-m3-plan.md M3-20): 값은 Host 가 정본이다 — 창은 칸의 글을 그대로 `db.settings.set` 에
// 보내고(범위를 스스로 막지 않는다) Host 가 거절한 까닭을 번역해 보인다. 가짜 Host 는 crates/pw-db/src/settings.rs 와 같은 범위로
// 판정한다(Host 쪽 판정 · 파일 쓰기 · 다음 실행의 상한은 crates/pw-db/tests/settings.rs).

import type { DbSettingItem, DbSettingsView, HostInfo } from "@pitwall/rpc-client";
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
import { settingsStrings } from "./strings";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const host = vi.hoisted(() => ({
  view: null as unknown as import("@pitwall/rpc-client").DbSettingsView,
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
          case "db.settings.get":
            return reply(host.view);
          case "db.settings.set": {
            const p = m.params as { key: string; value: unknown };
            const item = host.view.items.find((i) => i.key === p.key)!;
            if (p.value === null) {
              item.value = item.default;
              item.configured = false;
              return reply(structuredClone(host.view));
            }
            const ok =
              typeof p.value === "number" &&
              Number.isInteger(p.value) &&
              p.value >= item.min! &&
              p.value <= item.max!;
            if (!ok)
              return send({
                id: m.id,
                error: {
                  code: -32602,
                  message: "refused",
                  data: {
                    key: p.key,
                    reason:
                      typeof p.value === "number" && Number.isInteger(p.value)
                        ? { kind: "range", min: item.min, max: item.max }
                        : { kind: "notInteger" },
                  },
                },
              });
            item.value = p.value as number;
            item.configured = true;
            item.refused = null;
            return reply(structuredClone(host.view));
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

const T = settingsStrings(bundledTranslator("ko")).db;

function int(key: DbSettingItem["key"], value: number, min: number, max: number): DbSettingItem {
  const unit =
    key === "gridMaxRows"
      ? "rows"
      : key === "connectTimeout" || key === "redisResponseTimeout"
        ? "seconds"
        : "bytes";
  return {
    key,
    fileKey: `db.${key}`,
    unit,
    value,
    default: value,
    min,
    max,
    configured: false,
    refused: null,
  };
}

function view(): DbSettingsView {
  return {
    items: [
      int("gridMaxRows", 100_000, 1, 1_000_000),
      int("gridMaxBytes", 64 * 1024 * 1024, 1024 * 1024, 256 * 1024 * 1024),
      int("cellMaxBytes", 1024 * 1024, 1024, 16 * 1024 * 1024),
      int("redisResponseTimeout", 30, 1, 600),
      {
        key: "redisKeySeparator",
        fileKey: "db.redis_key_separator",
        unit: "text",
        value: ":",
        default: ":",
        min: null,
        max: null,
        configured: false,
        refused: null,
      },
      int("connectTimeout", 10, 1, 600),
    ],
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
  host.view = view();
  localStorage.clear();
});

afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 20)));
const section = () => document.querySelector('section[aria-labelledby="settings-db"]')!;
const row = (key: string) => section().querySelector<HTMLElement>(`[data-setting="${key}"]`)!;
const field = (key: string) => row(key).querySelector("input")!;
const sets = () => host.calls.filter((c) => c.method === "db.settings.set").map((c) => c.params);

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
  await vi.waitFor(() => expect(section()?.textContent).toContain(T.labels.gridMaxRows));
}

async function enter(key: string, text: string) {
  await act(async () => {
    const input = field(key);
    const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    set.call(input, text);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () => {
    row(key).querySelector("form")!.requestSubmit();
  });
  await settle();
}

async function click(key: string, label: string) {
  await act(async () => {
    [...row(key).querySelectorAll("button")].find((b) => b.textContent === label)!.click();
  });
  await settle();
}

it("shows every [db] item with its default and the range the host gave", async () => {
  await mount();
  for (const key of [
    "gridMaxRows",
    "gridMaxBytes",
    "cellMaxBytes",
    "redisResponseTimeout",
    "redisKeySeparator",
    "connectTimeout",
  ])
    expect(row(key), key).toBeTruthy();
  expect(field("gridMaxRows").value).toBe("100000");
  expect(row("gridMaxRows").textContent).toContain(T.rangeHint(1, 1_000_000, T.units.rows));
  expect(row("gridMaxBytes").textContent).toContain("64 MiB");
  expect(field("redisKeySeparator").value).toBe(":");
});

it("sends what was typed and shows the value the host kept", async () => {
  await mount();
  await enter("gridMaxRows", "5000");
  expect(sets()).toEqual([{ key: "gridMaxRows", value: 5000 }]);
  expect(field("gridMaxRows").value).toBe("5000");
  expect(row("gridMaxRows").querySelector("[role=alert]")).toBeNull();
  // 기본값으로: 그 키를 지운다(`null`).
  await click("gridMaxRows", T.reset);
  expect(sets()[1]).toEqual({ key: "gridMaxRows", value: null });
  expect(field("gridMaxRows").value).toBe("100000");
});

it("leaves the range to the host and shows why it refused", async () => {
  await mount();
  for (const [text, value, why] of [
    ["0", 0, T.range(1, 1_000_000)],
    ["-1", -1, T.range(1, 1_000_000)],
    ["1000000000000", 1_000_000_000_000, T.range(1, 1_000_000)],
    ["10.5", 10.5, T.notInteger],
    ["many", "many", T.notInteger],
  ] as const) {
    await enter("gridMaxRows", text);
    expect(sets().at(-1), text).toEqual({ key: "gridMaxRows", value });
    expect(row("gridMaxRows").querySelector("[role=alert]")?.textContent, text).toBe(why);
  }
  expect(host.view.items[0]!.value).toBe(100_000);
});

it("says when the file's value is not used", async () => {
  host.view.items[0] = {
    ...host.view.items[0]!,
    configured: true,
    refused: { kind: "range", min: 1, max: 1_000_000 },
  };
  await mount();
  expect(row("gridMaxRows").querySelector("[data-refused]")?.textContent).toBe(
    T.fileRefused("db.gridMaxRows", T.range(1, 1_000_000)),
  );
});
