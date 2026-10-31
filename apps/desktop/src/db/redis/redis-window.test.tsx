// @vitest-environment happy-dom
// Redis 키 문서(14-m3-plan M3-18 완료 기준의 창 시험): 프로젝트 창(App kind="project", `?project=p1`)을 가짜 Host 에 붙이고 Redis 탭을
// 연다. 본다: 해시 4 MiB 초과 값이 쪽으로 넘어간다(다음 쪽이 받은 `nextCursor` 로 `db.kv.get` 을 부른다); TTL 을 고치면
// `db.kv.setTtl` 이 그 값으로 불린다(받은 `3001` 의 확인을 거쳐); 콘솔의 `FLUSHALL` 이 거절 문장을 보인다; 대상이 있는 이름 바꾸기에
// 확인이 뜬다(`RENAMENX` 의 확인 → `targetExists` → 「덮어쓴다」 안내와 `RENAME` 의 확인); 탭이 열린 채 다시 시작하면 연결 RPC 가
// 불리지 않는다. Glide 는 canvas 라 셀을 기록하는 가짜로 바꾼다(src/db/grid/result-view-window.test.tsx 와 같다).

import type {
  ConfigView,
  DbKvConfirmRequired,
  DbSessionOpened,
  HostInfo,
  KeymapFile,
  KvKeyInfo,
  KvValuePage,
  Project,
  ProjectTrust,
  RuntimeInfo,
} from "@pitwall/rpc-client";
import type { DockviewApi } from "dockview-react";
import { forwardRef, StrictMode, useImperativeHandle } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, vi } from "vitest";

import defaultTheme from "../../../../../extensions/theme-default/themes/default.json";
import { App } from "../../App";
import { openTab, tabParams } from "../../editor/editor-tabs";
import { bundledTranslator } from "../../i18n/testing";
import type { KeymapNative } from "../../keymap/native";
import { act, endTest, it } from "../../test-support/ended-test";
import { createThemeStore } from "../../theme/theme-store";
import { ThemeStoreProvider } from "../../theme/theme-switcher";
import { parseTheme } from "../../theme/tokens";
import { type DbDocumentSpec, dbDocumentKey } from "../documents/spec";
import { dbSafetyStrings } from "../safety/strings";
import { dbRedisStrings } from "./strings";
import { PAGE_COUNT } from "./value-view";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const MIB = 1024 * 1024;

type Call = { method: string; params: Record<string, unknown> };

const host = vi.hoisted(() => ({
  calls: [] as { method: string; params: Record<string, unknown> }[],
  /** 프로젝트 상태(`project.state.*`). */
  state: null as unknown,
  /** 스캔이 줄 키. */
  keys: [] as unknown[],
  /** `db.kv.get` 의 답(키 · 커서마다). */
  pages: new Map<string, unknown>(),
  /** 이름 바꾸기 대상이 있는가. */
  targetExists: true,
  /** 세션을 열 때 비밀이 필요하다(3101) — `db.secret.set` 이 오면 풀린다. */
  needSecret: false,
  /** 있으면 `db.session.open` 의 답을 이것이 풀릴 때까지 붙잡는다. */
  holdOpen: undefined as Promise<void> | undefined,
}));

const glide = vi.hoisted(() => ({
  rows: [] as number[],
  cells: [] as ((cell: [number, number]) => { data?: string; displayData?: string })[],
}));

vi.mock("@glideapps/glide-data-grid", () => ({
  CompactSelection: { empty: () => ({ toArray: () => [] }) },
  GridCellKind: { Text: "text", Loading: "loading" },
  DataEditor: forwardRef(function FakeEditor(
    props: {
      rows: number;
      getCellContent: (cell: [number, number]) => { data?: string; displayData?: string };
    },
    ref,
  ) {
    glide.rows.push(props.rows);
    glide.cells.push(props.getCellContent);
    useImperativeHandle(ref, () => ({ focus: () => {} }));
    return <button type="button" data-testid="fake-glide" />;
  }),
}));

vi.mock("../../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../../keymap/default-keymap");
  const { i18nState } = await import("../../i18n/testing");
  return {
    connectHost: async () => {
      const [client, server] = pair();
      let next = 1;
      const streams = new Map<number, unknown[]>();
      const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
      const opened = {
        session: {
          sessionId: "s1",
          projectId: "p1",
          connectionId: "redis-local",
          mode: "auto",
          state: { kind: "idle" },
        },
        name: "redis-local",
        readOnly: false,
        production: false,
        driver: { id: "redis", dialect: null, supports: {} },
        server: { version: "7.2.10", tls: "plaintext" },
      } as unknown as DbSessionOpened;
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const params = (m.params ?? {}) as Record<string, unknown>;
        host.calls.push({ method: m.method, params });
        const reply = (result: unknown) => send({ id: m.id, result });
        const fail = (code: number, message: string, data: unknown) =>
          send({ id: m.id, error: { code, message, data } });
        const confirmRequired = (argv: string[], hash: string) =>
          fail(3001, "confirm before running this command", {
            grade: "unboundedWrite",
            action: m.method === "db.kv.setTtl" ? "setTtl" : "rename",
            argv,
            statementsHash: hash,
            nameRequired: false,
            rejected: null,
          } satisfies DbKvConfirmRequired);
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
              state: "trusted",
            } satisfies ProjectTrust);
          case "project.claimTrustPrompt":
            return reply(false);
          case "project.storage":
            return reply({ kind: "ok", path: "/data/state.db" });
          case "project.state.get":
            return reply({ state: host.state, problem: null });
          case "project.state.set":
            host.state = structuredClone((params as { state: unknown }).state);
            return reply(null);
          case "runtime.get":
            return reply({
              provider: "shell",
              source: { kind: "loginShell", shell: "/bin/zsh" },
              status: { kind: "ok" },
              tools: [],
            } satisfies RuntimeInfo);
          case "events.subscribe":
            return reply({ subscriptionId: next++ });
          case "events.unsubscribe":
            return reply(null);
          case "db.connection.get":
            return reply({
              id: "redis-local",
              config: { name: "redis-local", driver: "redis", host: "localhost", port: 6379 },
              layers: [],
              readOnly: false,
              state: { kind: "ready" },
            });
          case "db.secret.set":
            host.needSecret = false;
            return reply(null);
          case "db.session.open":
            if (host.holdOpen && !host.needSecret) {
              const held = host.holdOpen;
              host.holdOpen = undefined;
              void held.then(() => reply(opened));
              return;
            }
            if (host.needSecret)
              return fail(3101, "db connection `redis-local` needs its password", {
                connectionId: "redis-local",
                key: "password",
                reason: "missing",
                target: {
                  driver: "redis",
                  host: "localhost",
                  port: 6379,
                  user: null,
                  tunnel: null,
                  sslmode: "disable",
                  sslrootcert: null,
                  sslcert: null,
                  sslkey: null,
                },
                previous: null,
              });
            return reply(opened);
          case "db.session.close":
            return reply(null);
          case "db.kv.scan": {
            const streamId = next++;
            streams.set(streamId, [
              { kind: "accepted", executionId: "x1" },
              { kind: "keys", keys: host.keys },
              { kind: "done", keys: host.keys.length, elapsedMs: 3 },
            ]);
            return reply({ streamId });
          }
          case "stream.credit": {
            const { streamId } = params as { streamId: number };
            const chunks = streams.get(streamId);
            if (!chunks) return;
            streams.delete(streamId);
            for (const data of chunks) send({ method: "stream.chunk", params: { streamId, data } });
            return send({ method: "stream.end", params: { streamId, status: "completed" } });
          }
          case "stream.cancel":
            return;
          case "db.kv.get": {
            const { key, range } = params as { key: string; range: { cursor: string | null } };
            const page = host.pages.get(`${key}@${range?.cursor ?? ""}`);
            return page ? reply(page) : fail(-32005, "no page", null);
          }
          case "db.kv.setTtl": {
            const { key, ttlMs, confirmed } = params as {
              key: string;
              ttlMs: number;
              confirmed: unknown;
            };
            if (!confirmed) return confirmRequired(["PEXPIRE", key, String(ttlMs)], "h-ttl");
            return reply(true);
          }
          case "db.kv.rename": {
            const { from, to, overwrite, confirmed } = params as {
              from: string;
              to: string;
              overwrite: boolean;
              confirmed: unknown;
            };
            if (!confirmed)
              return confirmRequired(
                [overwrite ? "RENAME" : "RENAMENX", from, to],
                overwrite ? "h-rename" : "h-renamenx",
              );
            return reply(!overwrite && host.targetExists ? "targetExists" : "renamed");
          }
          case "db.kv.parseCommand": {
            const text = String((params as { text: string }).text);
            // 닫는 따옴표 뒤에 글자가 붙은 줄은 Host 처럼 까닭 코드와 함께 거절한다.
            if (/"[^"]*"\S/.test(text))
              return fail(-32602, "invalid params: a closing quote must be followed by a space", {
                reason: "closingQuote",
              });
            return reply({ argv: text.split(/\s+/) });
          }
          case "db.kv.command": {
            const { argv } = params as { argv: string[] };
            if (argv[0]?.toUpperCase() === "FLUSHALL")
              return fail(-32001, "`FLUSHALL` is not allowed in the console", {
                reason: "blockedCommand",
                block: "notAllowed",
                command: "FLUSHALL",
              });
            return reply({ status: "OK" });
          }
          default:
            return fail(-32601, `no ${m.method}`, null);
        }
      });
      return { transport: client, close: async () => {} };
    },
  };
});

const R = dbRedisStrings(bundledTranslator("ko"));
const S = dbSafetyStrings(bundledTranslator("ko"));

const native: KeymapNative = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
  closeWindow: async () => {},
};

const SPEC: DbDocumentSpec = {
  kind: "redis",
  tabId: "tab-r1",
  connectionId: "redis-local",
  database: "0",
};

function info(key: string, kind: KvKeyInfo["kind"], over: Partial<KvKeyInfo> = {}): KvKeyInfo {
  return { key, kind, ttlMs: null, size: 10, ...over };
}

let root: Root | undefined;
let dock: DockviewApi | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(console, "info").mockImplementation(() => {});
  vi.spyOn(console, "warn").mockImplementation(() => {});
  // 키 트리는 가상화한다 — happy-dom 은 높이를 계산하지 않으므로 트리에 보이는 높이를 준다.
  for (const prop of ["offsetHeight", "clientHeight"] as const)
    vi.spyOn(HTMLElement.prototype, prop, "get").mockImplementation(function (this: HTMLElement) {
      return this.getAttribute("role") === "tree" ? 600 : 0;
    });
  window.history.replaceState(null, "", "/?window=project&project=p1");
  localStorage.clear();
  host.calls = [];
  host.state = null;
  host.keys = [];
  host.pages = new Map();
  host.targetExists = true;
  host.needSecret = false;
  host.holdOpen = undefined;
  glide.rows = [];
  glide.cells = [];
});

afterEach(async () => {
  await endTest();
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 30)));

async function mountApp(strict = false) {
  dock = undefined;
  const element = document.createElement("div");
  document.body.append(element);
  const store = createThemeStore({
    system: { current: () => "light", subscribe: () => () => {} },
    storage: { read: () => undefined, write: () => {} },
    theme: parseTheme(defaultTheme),
    apply: () => {},
  });
  root = createRoot(element);
  const app = (
    <ThemeStoreProvider store={store}>
      <App kind="project" native={native} onDockApi={(api) => (dock = api)} />
    </ThemeStoreProvider>
  );
  // 앱은 StrictMode 로 그린다(src/main.tsx) — 개발 빌드처럼 effect 를 한 번 걷고 다시 돌린다.
  await act(async () => root!.render(strict ? <StrictMode>{app}</StrictMode> : app));
  await settle();
}

const $ = <T extends Element = HTMLElement>(selector: string) =>
  document.querySelector<T>(selector);
const calls = (method: string): Call[] => host.calls.filter((c) => c.method === method);

async function click(el: Element | null) {
  expect(el).not.toBeNull();
  await act(async () => (el as HTMLElement).click());
  await settle();
}

async function type(input: HTMLInputElement | null, value: string) {
  expect(input).not.toBeNull();
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
  await act(async () => {
    setter.call(input, value);
    input!.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

async function enter(input: HTMLInputElement | null) {
  await act(async () =>
    input!.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })),
  );
  await settle();
}

/** Redis 탭을 열고 [연결] 해 스캔이 끝난 데까지. */
async function openConnected() {
  await mountApp();
  await act(async () => {
    openTab(dock!, { path: dbDocumentKey(SPEC), db: SPEC, title: "redis-local · DB 0" });
  });
  await settle();
  await click($("[data-redis-document] [data-action=connect]"));
  await vi.waitFor(() => expect($("[data-testid=redis-scan-status]")?.dataset.scan).toBe("done"));
}

async function openKey(name: string) {
  const row = $(`[data-testid=redis-key-tree] [data-key="${name}"]`);
  expect(row).not.toBeNull();
  await act(async () => row!.dispatchEvent(new MouseEvent("dblclick", { bubbles: true })));
  await settle();
}

const confirmDialog = () => $("[data-testid=db-risk-confirm]");

describe("the Redis key document", () => {
  it("pages a hash over 4 MiB with the cursor the Host returned", async () => {
    host.keys = [info("big", "hash", { size: 6 * MIB })];
    // 첫 쪽: Host 의 4 MiB 예산에 닿아 끊은 쪽(1 MiB 값 다섯) — 다음 쪽 커서가 있다.
    const mib = "v".repeat(MIB);
    host.pages.set("big@", {
      value: {
        hash: Array.from({ length: 5 }, (_, i) => ({
          field: { text: `f${i}` },
          value: { text: mib },
        })),
      },
      total: 7,
      nextCursor: "29",
    } satisfies KvValuePage);
    host.pages.set("big@29", {
      value: {
        hash: [
          { field: { text: "f5" }, value: { text: "x" } },
          { field: { text: "f6" }, value: { text: "y" } },
        ],
      },
      total: 7,
      nextCursor: null,
    } satisfies KvValuePage);
    await openConnected();
    await openKey("big");
    expect(calls("db.kv.get")).toEqual([
      expect.objectContaining({
        params: expect.objectContaining({
          sessionId: "s1",
          key: "big",
          range: { cursor: null, count: PAGE_COUNT },
        }),
      }),
    ]);
    expect($("[data-testid=redis-page]")?.textContent).toBe(R.viewer.page(1));
    expect(glide.rows.at(-1)).toBe(5);
    expect($<HTMLButtonElement>("[data-action=next-page]")!.disabled).toBe(false);
    await click($("[data-action=next-page]"));
    expect(calls("db.kv.get").at(-1)!.params.range).toEqual({ cursor: "29", count: PAGE_COUNT });
    expect($("[data-testid=redis-page]")?.textContent).toBe(R.viewer.page(2));
    expect(glide.rows.at(-1)).toBe(2);
    expect(glide.cells.at(-1)!([0, 0]).data).toBe("f5");
    expect($<HTMLButtonElement>("[data-action=next-page]")!.disabled).toBe(true);
    // 앞 쪽은 지나온 커서로 돌아간다.
    await click($("[data-action=prev-page]"));
    expect(calls("db.kv.get").at(-1)!.params.range).toEqual({ cursor: null, count: PAGE_COUNT });
    expect($("[data-testid=redis-page]")?.textContent).toBe(R.viewer.page(1));
  });

  it("reads a value only when the key is opened, not when it is picked", async () => {
    host.keys = [info("a", "string"), info("b", "string")];
    host.pages.set("b@", { value: { string: { text: "1" } }, total: 1, nextCursor: null });
    await openConnected();
    await click($("[data-testid=redis-key-tree] [data-key=a]"));
    await act(async () =>
      $("[data-testid=redis-key-tree]")!.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
      ),
    );
    await settle();
    expect($("[data-testid=redis-key-tree] [data-key=b]")?.getAttribute("aria-selected")).toBe(
      "true",
    );
    expect(calls("db.kv.get")).toEqual([]);
    expect($("[data-testid=redis-value]")).toBeNull();
    await act(async () =>
      $("[data-testid=redis-key-tree]")!.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
      ),
    );
    await settle();
    expect(calls("db.kv.get").map((c) => c.params.key)).toEqual(["b"]);
    expect($("[data-testid=redis-value]")?.dataset.key).toBe("b");
  });

  it("calls db.kv.setTtl with the typed TTL, through the confirmation the Host asked for", async () => {
    host.keys = [info("session:1", "string", { ttlMs: 60_000 })];
    host.pages.set("session:1@", { value: { string: { text: "v" } }, total: 1, nextCursor: null });
    await openConnected();
    await click($("[data-group='session:']"));
    await openKey("session:1");
    await click($("[data-testid=redis-value] [data-action=edit-ttl]"));
    const input = $<HTMLInputElement>("[data-testid=redis-key-prompt-input]");
    await type(input, "120");
    await click($("[data-testid=redis-key-prompt] [data-action=apply]"));
    // 확인 없이 한 번 → 3001 → 다이얼로그(서버에 갈 명령) → [실행] 이 받은 해시로 다시.
    expect(calls("db.kv.setTtl")).toHaveLength(1);
    expect(calls("db.kv.setTtl")[0]!.params).toMatchObject({
      sessionId: "s1",
      key: "session:1",
      ttlMs: 120_000,
      confirmed: null,
    });
    expect(confirmDialog()).not.toBeNull();
    expect(
      [...document.querySelectorAll("[data-testid=db-confirm-kv-arg]")].map((a) => a.textContent),
    ).toEqual(['"PEXPIRE"', '"session:1"', '"120000"']);
    await click($("[data-testid=db-risk-confirm] [data-action=run]"));
    expect(calls("db.kv.setTtl")).toHaveLength(2);
    expect(calls("db.kv.setTtl")[1]!.params).toMatchObject({
      key: "session:1",
      ttlMs: 120_000,
      confirmed: { grade: "unboundedWrite", statementsHash: "h-ttl", name: null },
    });
    expect(confirmDialog()).toBeNull();
    expect($("[data-testid=redis-ttl]")?.textContent).toBe(R.viewer.ttl("2 m 0 s"));
  });

  it("shows the refusal sentence for FLUSHALL in the console, with no confirmation", async () => {
    await openConnected();
    const input = $<HTMLInputElement>("[data-testid=redis-console-input]");
    await type(input, "FLUSHALL");
    await enter(input);
    expect(calls("db.kv.parseCommand")[0]!.params).toEqual({ text: "FLUSHALL" });
    expect(calls("db.kv.command")[0]!.params).toMatchObject({
      sessionId: "s1",
      argv: ["FLUSHALL"],
    });
    const refusal = $("[data-testid=redis-console-refusal]");
    expect(refusal?.dataset.reason).toBe("notAllowed");
    expect(refusal?.textContent).toBe(R.console.blocked.notAllowed("FLUSHALL"));
    expect(confirmDialog()).toBeNull();
    expect(calls("db.kv.command")).toHaveLength(1);
  });

  // 재리뷰 F4: 나누지 못한 까닭은 Host 의 코드를 번역해 기록에 보이고, 명령을 보내지 않는다.
  it("shows why a console line could not be split, in the window's language", async () => {
    await openConnected();
    const input = $<HTMLInputElement>("[data-testid=redis-console-input]");
    await type(input, 'SET k "a"b');
    await enter(input);
    await vi.waitFor(() => expect($("[data-testid=redis-console-error]")).not.toBeNull());
    expect($("[data-testid=redis-console-error]")!.textContent).toBe(
      R.console.parseFailed(R.console.parseRefused.closingQuote),
    );
    expect($("[data-testid=redis-console-error]")!.textContent).not.toContain("closing quote");
    expect(calls("db.kv.command")).toEqual([]);
  });

  it("asks to overwrite when the rename target exists", async () => {
    host.keys = [info("a", "string"), info("b", "string")];
    host.pages.set("a@", { value: { string: { text: "1" } }, total: 1, nextCursor: null });
    host.pages.set("b@", { value: { string: { text: "1" } }, total: 1, nextCursor: null });
    await openConnected();
    await openKey("a");
    await click($("[data-testid=redis-value] [data-action=rename]"));
    await type($<HTMLInputElement>("[data-testid=redis-key-prompt-input]"), "b");
    await click($("[data-testid=redis-key-prompt] [data-action=apply]"));
    // 덮지 않는 이름 바꾸기(RENAMENX)도 분류대로 확인이다.
    expect(confirmDialog()).not.toBeNull();
    expect($("[data-testid=db-confirm-note]")).toBeNull();
    expect(
      [...document.querySelectorAll("[data-testid=db-confirm-kv-arg]")].map((a) => a.textContent),
    ).toEqual(['"RENAMENX"', '"a"', '"b"']);
    await click($("[data-testid=db-risk-confirm] [data-action=run]"));
    // 대상이 있었다 → 「덮어쓴다」 안내와 함께 RENAME 의 확인.
    expect(calls("db.kv.rename").map((c) => c.params.overwrite)).toEqual([false, false, true]);
    expect(confirmDialog()).not.toBeNull();
    expect($("[data-testid=db-confirm-note]")?.textContent).toBe(R.key.overwriteNote("b"));
    expect(
      [...document.querySelectorAll("[data-testid=db-confirm-kv-arg]")].map((a) => a.textContent),
    ).toEqual(['"RENAME"', '"a"', '"b"']);
    expect($("[data-testid=db-risk-confirm]")?.textContent).toContain(S.confirm.kv.command);
    await click($("[data-testid=db-risk-confirm] [data-action=run]"));
    expect(calls("db.kv.rename").at(-1)!.params).toMatchObject({
      from: "a",
      to: "b",
      overwrite: true,
      confirmed: { statementsHash: "h-rename" },
    });
    expect($("[data-testid=redis-key-tree] [data-key=a]")).toBeNull();
    expect($("[data-testid=redis-value]")?.dataset.key).toBe("b");
  });

  it("cancelling the overwrite confirmation leaves both keys", async () => {
    host.keys = [info("a", "string"), info("b", "string")];
    host.pages.set("a@", { value: { string: { text: "1" } }, total: 1, nextCursor: null });
    await openConnected();
    await openKey("a");
    await click($("[data-testid=redis-value] [data-action=rename]"));
    await type($<HTMLInputElement>("[data-testid=redis-key-prompt-input]"), "b");
    await click($("[data-testid=redis-key-prompt] [data-action=apply]"));
    await click($("[data-testid=db-risk-confirm] [data-action=run]"));
    await click($("[data-testid=db-risk-confirm] [data-action=cancel]"));
    expect(calls("db.kv.rename").map((c) => c.params.overwrite)).toEqual([false, false, true]);
    expect($("[data-testid=redis-key-tree] [data-key=a]")).not.toBeNull();
    expect($("[data-testid=redis-key-tree] [data-key=b]")).not.toBeNull();
  });

  it("asks for the password the Host needs and connects with it", async () => {
    host.needSecret = true;
    host.keys = [info("a", "string")];
    await mountApp();
    await act(async () => {
      openTab(dock!, { path: dbDocumentKey(SPEC), db: SPEC, title: "redis-local · DB 0" });
    });
    await settle();
    await click($("[data-redis-document] [data-action=connect]"));
    expect($("[data-testid=db-access-prompt]")).not.toBeNull();
    await type($<HTMLInputElement>("[data-testid=db-access-secret]"), "pw-1");
    await click($("[data-testid=db-access-prompt] [data-action=run]"));
    await vi.waitFor(() => expect($("[data-testid=redis-scan-status]")?.dataset.scan).toBe("done"));
    expect(calls("db.secret.set")[0]!.params).toMatchObject({
      connectionId: "redis-local",
      key: "password",
      value: "pw-1",
    });
    expect(calls("db.session.open")).toHaveLength(2);
    expect($("[data-testid=redis-connect-error]")).toBeNull();
  });

  // 재리뷰 F1: StrictMode 에서도 [연결] 이 연 세션은 닫히지 않고 스캔한다.
  it("connects and scans under StrictMode", async () => {
    host.keys = [info("a", "string")];
    await mountApp(true);
    await act(async () => {
      openTab(dock!, { path: dbDocumentKey(SPEC), db: SPEC, title: "redis-local · DB 0" });
    });
    await settle();
    await click($("[data-redis-document] [data-action=connect]"));
    await vi.waitFor(() => expect($("[data-testid=redis-scan-status]")?.dataset.scan).toBe("done"));
    expect(calls("db.session.open")).toHaveLength(1);
    expect(calls("db.session.close")).toEqual([]);
    expect(calls("db.kv.scan")).toHaveLength(1);
  });

  async function openTabAndConnect() {
    await mountApp();
    await act(async () => {
      openTab(dock!, { path: dbDocumentKey(SPEC), db: SPEC, title: "redis-local · DB 0" });
    });
    await settle();
    await click($("[data-redis-document] [data-action=connect]"));
  }

  const closeRedisTab = () =>
    act(async () => dock!.panels.find((p) => tabParams(p).db !== undefined)!.api.close());

  // 리뷰 L2: 연결 중에 탭을 닫으면 돌아온 세션을 곧바로 닫고 스캔하지 않는다.
  it("closes a session that opens after its tab was closed, and does not scan", async () => {
    host.keys = [info("a", "string")];
    let release!: () => void;
    host.holdOpen = new Promise<void>((r) => (release = r));
    await openTabAndConnect();
    expect(calls("db.session.open")).toHaveLength(1);
    await closeRedisTab();
    expect($("[data-redis-document]")).toBeNull();
    await act(async () => release());
    await vi.waitFor(() => expect(calls("db.session.close")).toHaveLength(1));
    expect(calls("db.session.close")[0]!.params).toEqual({ sessionId: "s1" });
    await settle();
    expect(calls("db.kv.scan")).toEqual([]);
  });

  // M3-13 의 접근 묻기는 주인이 언마운트되면 거짓으로 끝난다 — 묻는 중에 탭을 닫으면 다시 열지 않는다.
  it("does not open again when the tab closes while the password is asked", async () => {
    host.needSecret = true;
    await openTabAndConnect();
    expect($("[data-testid=db-access-prompt]")).not.toBeNull();
    await closeRedisTab();
    await settle();
    expect($("[data-testid=db-access-prompt]")).toBeNull();
    expect(calls("db.session.open")).toHaveLength(1);
    expect(calls("db.secret.set")).toEqual([]);
    expect(calls("db.kv.scan")).toEqual([]);
  });

  it("does not call a connection RPC when restarted with the tab open", async () => {
    host.keys = [info("a", "string")];
    await openConnected();
    expect(calls("db.session.open")).toHaveLength(1);
    await vi.waitFor(
      () =>
        expect((host.state as { main?: { tabs: { db?: unknown }[] } } | null)?.main?.tabs).toEqual([
          expect.objectContaining({ db: SPEC }),
        ]),
      { timeout: 2000 },
    );
    // 다시 시작(창의 웹뷰가 새로 뜬다).
    act(() => root?.unmount());
    root = undefined;
    document.body.innerHTML = "";
    host.calls = [];
    await mountApp();
    await vi.waitFor(() => expect($("[data-redis-document]")).not.toBeNull());
    await settle();
    expect($("[data-redis-document] [data-action=connect]")).not.toBeNull();
    expect($("[data-testid=redis-not-connected]")).not.toBeNull();
    // `db.session.unattendedRollbacks` 는 창이 열릴 때 콘솔 기능(M3-15)이 읽는 Host 의 기록이다 — 연결을 열지 않는다.
    expect(
      host.calls
        .map((c) => c.method)
        .filter((m) => /^db\.(session|kv|query)\./.test(m))
        .filter((m) => m !== "db.session.unattendedRollbacks"),
    ).toEqual([]);
    // 연결 설정은 읽는다(표지 — 연결을 열지 않는다).
    expect(calls("db.connection.get")).toHaveLength(1);
  });
});
