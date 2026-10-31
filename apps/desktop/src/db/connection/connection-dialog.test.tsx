// @vitest-environment happy-dom
// 연결 편집 다이얼로그(14-m3-plan.md M3-14 완료 기준)를 가짜 Host 로 본다: 새 원격 PG 연결의 처음 sslmode 가 verify-full,
// localhost 면 disable, 터널 + DB 호스트 db.internal 이면 verify-full; require · prefer 를 고르면 경고 문장; 드라이버가 받지 않는
// sslmode 는 흐리고 까닭이 있다; 저장 요청 · 알림 · 콘솔 · DOM 에 비밀번호가 없고 값은 `db.secret.set` 에만 간다(다시 열면 「저장됨」
// 만); [연결 테스트] 는 저장 RPC 를 부르지 않고 초안으로 돈다(파일 mtime 은 crates/pw-db/tests/dialog.rs); 저장 위치; stateProblem 을
// 비밀번호 칸보다 먼저 보인다; 미신뢰 프로젝트; SSH 호스트 고르기; Host 의 거절 글; 액션 id.

import { LocalizationProvider } from "@fluent/react";
import {
  type DbConnectionConfig,
  type DbConnectionEntry,
  type DbConnectionList,
  type DbDriverEntry,
  ERROR_CODE,
  RpcCallError,
  type RpcClient,
  type SslMode,
} from "@pitwall/rpc-client";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, beforeEach, describe, expect, vi } from "vitest";

import { bundledLocalization, bundledTranslator } from "../../i18n/testing";
import type { NotificationInput } from "../../notifications/notifications";
import { act, endTest, it } from "../../test-support/ended-test";
import { ConnectionDialog, type ConnectionDialogDeps } from "./connection-dialog";
import { connectionActions, editArgs } from "./connection-feature";
import { dbConnectionStrings } from "./strings";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

vi.mock("../../keymap/keymap-provider", () => ({ useKeymapPopup: () => {} }));

// happy-dom 21 에는 표준 속성 `HTMLSelectElement.size`(기본 0) · `HTMLOptionElement.defaultSelected`(`selected` 속성)가 없다.
// react-hook-form 은 이 둘로 「준비되기 전에 사람이 고른 select」 를 가려(`isSelectEdited`) 그렇다고 보면 기본값 대신 첫 옵션을
// 읽는다 — WebKit · Chromium 에서는 일어나지 않는 일이라 표준대로 채운다.
beforeAll(() => {
  if (!("size" in HTMLSelectElement.prototype))
    Object.defineProperty(HTMLSelectElement.prototype, "size", {
      configurable: true,
      get(this: HTMLSelectElement) {
        return Number(this.getAttribute("size") ?? 0);
      },
    });
  if (!("defaultSelected" in HTMLOptionElement.prototype))
    Object.defineProperty(HTMLOptionElement.prototype, "defaultSelected", {
      configurable: true,
      get(this: HTMLOptionElement) {
        return this.hasAttribute("selected");
      },
    });
});

const T = dbConnectionStrings(bundledTranslator("ko"));
const SECRET = "pw-dialog-secret-5e1b77";
const ALL_SSL: SslMode[] = ["disable", "allow", "prefer", "require", "verify-ca", "verify-full"];

function driver(kind: string, sslModes: SslMode[]): DbDriverEntry {
  return {
    kind,
    capabilities: {
      sql: kind !== "redis",
      kv: kind === "redis",
      serverCursor: false,
      cancel: false,
      transactions: false,
      savepoints: false,
      readOnlyTransaction: false,
      explain: false,
      metadata: false,
      schemas: false,
      multipleDatabases: false,
      sslModes,
    },
  };
}

const DRIVERS: DbDriverEntry[] = [
  driver("mysql", ["disable", "allow", "prefer", "require", "verify-full"]),
  driver("postgres", ALL_SSL),
  driver("redis", ["disable", "require", "verify-full"]),
  driver("sqlite", []),
];

interface Call {
  method: string;
  params: Record<string, unknown>;
}

type Answer = (params: Record<string, unknown>) => unknown;

let calls: Call[] = [];
let answers: Record<string, Answer> = {};
let notes: NotificationInput[] = [];
let consoleLines: string[] = [];
let root: Root | undefined;

function list(over: Partial<DbConnectionList> = {}): DbConnectionList {
  return { connections: [], problem: null, stateProblem: null, trusted: true, ...over };
}

function entryOf(
  id: string,
  config: DbConnectionConfig,
  over: Partial<DbConnectionEntry> = {},
): DbConnectionEntry {
  return { id, config, layers: ["shared"], readOnly: false, state: { kind: "ready" }, ...over };
}

function client(): RpcClient {
  return {
    call: async (method: string, params: Record<string, unknown>) => {
      calls.push({ method, params: structuredClone(params ?? {}) });
      const answer = answers[method];
      if (!answer) throw new Error(`unexpected call ${method}`);
      const value = await answer(params ?? {});
      if (value instanceof RpcCallError) throw value;
      return value;
    },
  } as unknown as RpcClient;
}

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  calls = [];
  notes = [];
  consoleLines = [];
  answers = {
    "db.driver.list": () => DRIVERS,
    "db.connection.list": () => list(),
    "db.ssh.hosts": () => ({ hosts: ["bastion", "jump-eu"] }),
    "db.connection.upsert": (p) =>
      entryOf(p.connectionId as string, p.config as DbConnectionConfig, {
        layers: [p.location === "local" ? "local" : "shared"],
        state: { kind: "secretRequired", reason: "missing", target: null as never, previous: null },
      }),
    "db.secret.set": () => null,
    "db.secret.clear": () => null,
    "db.connection.test": () => ({ version: "17.6", tls: "verified" }),
  };
  for (const level of ["log", "info", "warn", "error", "debug"] as const) {
    vi.spyOn(console, level).mockImplementation((...args: unknown[]) => {
      consoleLines.push(
        args
          .map((a) => (a instanceof Error ? a.message : (JSON.stringify(a) ?? String(a))))
          .join(" "),
      );
    });
  }
});

afterEach(async () => {
  await endTest();
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

async function open(connectionId?: string, closed: () => void = () => {}) {
  const deps: ConnectionDialogDeps = {
    client: client(),
    projectId: "p1",
    worktree: "/work/wt",
    notify: (n) => notes.push(n),
  };
  const element = document.createElement("div");
  document.body.append(element);
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <LocalizationProvider l10n={bundledLocalization("ko")}>
        <ConnectionDialog
          deps={deps}
          {...(connectionId !== undefined && { connectionId })}
          onClose={closed}
        />
      </LocalizationProvider>,
    ),
  );
  // 스키마(동적 import) · 목록을 읽는다.
  for (let i = 0; i < 200 && !form(); i++) await act(() => new Promise((r) => setTimeout(r, 10)));
  if (!form())
    throw new Error(
      `no form: ${document.body.textContent} ${consoleLines.join("\n").slice(0, 1500)}`,
    );
}

const form = () => document.querySelector<HTMLFormElement>("[data-testid=db-connection-form]");
const field = <E extends HTMLElement = HTMLInputElement>(name: string) =>
  document.querySelector<E>(`[data-field="${name}"]`);
const testid = <E extends HTMLElement = HTMLElement>(id: string) =>
  document.querySelector<E>(`[data-testid="${id}"]`);

async function type(el: HTMLInputElement | null, value: string) {
  expect(el).toBeTruthy();
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
  await act(async () => {
    setter.call(el, value);
    el!.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

async function choose(el: HTMLSelectElement | null, value: string) {
  expect(el).toBeTruthy();
  await act(async () => {
    el!.value = value;
    el!.dispatchEvent(new Event("change", { bubbles: true }));
  });
}

async function click(el: HTMLElement | null) {
  expect(el).toBeTruthy();
  await act(async () => el!.click());
  await act(() => new Promise((r) => setTimeout(r, 0)));
}

async function tab(id: string) {
  await click(document.querySelector<HTMLElement>(`[role=tab][data-tab="${id}"]`));
}

async function submit() {
  await click(testid("db-conn-save"));
  await act(() => new Promise((r) => setTimeout(r, 0)));
}

const sslmode = () => field<HTMLSelectElement>("sslmode")!.value;
const calledWith = (method: string) => calls.filter((c) => c.method === method);

describe("sslmode default (decision M3-D10)", () => {
  it("a new remote PG starts at verify-full, localhost gives disable, a tunnel to db.internal gives verify-full", async () => {
    await open();
    expect(field<HTMLSelectElement>("driver")!.value).toBe("postgres");
    expect(sslmode()).toBe("verify-full");
    await type(field("host"), "db.example.com");
    expect(sslmode()).toBe("verify-full");
    await type(field("host"), "localhost");
    expect(sslmode()).toBe("disable");
    await tab("ssh");
    await click(field("tunnel"));
    await type(field("host"), "db.internal");
    expect(sslmode()).toBe("verify-full");
    // 터널이 있어도 루프백은 바스티언 기준의 루프백이다.
    await type(field("host"), "127.0.0.1");
    expect(sslmode()).toBe("disable");
  });

  it("stops following the host once the person picks a mode", async () => {
    await open();
    await tab("ssl");
    await choose(field("sslmode"), "verify-ca");
    await type(field("host"), "localhost");
    expect(sslmode()).toBe("verify-ca");
  });
});

describe("sslmode warnings and the driver's modes", () => {
  it("require and prefer warn that the server certificate is not verified", async () => {
    await open();
    await tab("ssl");
    expect(testid("db-conn-ssl-warning")).toBeNull();
    for (const mode of ["require", "prefer", "allow"] as const) {
      await choose(field("sslmode"), mode);
      const warning = testid("db-conn-ssl-warning");
      expect(warning?.dataset.warning, mode).toBe("unverified");
      expect(warning?.textContent).toBe(T.ssl.unverified);
    }
    await choose(field("sslmode"), "verify-full");
    expect(testid("db-conn-ssl-warning")).toBeNull();
  });

  it("dims the modes a driver does not take and says why", async () => {
    await open();
    await tab("ssl");
    const disabled = () =>
      [...field<HTMLSelectElement>("sslmode")!.options]
        .filter((o) => o.disabled)
        .map((o) => o.value);
    expect(disabled()).toEqual([]);
    await choose(field("driver"), "mysql");
    expect(disabled()).toEqual(["verify-ca"]);
    const option = [...field<HTMLSelectElement>("sslmode")!.options].find(
      (o) => o.value === "verify-ca",
    )!;
    expect(option.title).toBe(T.ssl.unsupportedMysqlVerifyCa);
    expect(testid("db-conn-ssl-unsupported-list")?.textContent).toContain(
      T.ssl.unsupportedMysqlVerifyCa,
    );
    await choose(field("driver"), "redis");
    expect(disabled()).toEqual(["allow", "prefer", "verify-ca"]);
    expect(
      [...field<HTMLSelectElement>("sslmode")!.options].find((o) => o.value === "prefer")!.title,
    ).toBe(T.ssl.unsupported(T.field.driverName("redis"), "prefer"));
  });

  it("refuses to save a mode the driver does not take", async () => {
    await open();
    await type(field("name"), "cache");
    await type(field("host"), "cache.internal");
    await tab("ssl");
    await choose(field("sslmode"), "prefer");
    await choose(field("driver"), "redis");
    expect(testid("db-conn-ssl-unsupported")).toBeTruthy();
    expect(testid<HTMLButtonElement>("db-conn-save")!.disabled).toBe(true);
    expect(testid<HTMLButtonElement>("db-conn-test")!.disabled).toBe(true);
  });

  it("does not dim anything when the driver list could not be read (the host judges)", async () => {
    answers["db.driver.list"] = () =>
      new RpcCallError({ code: ERROR_CODE.INTERNAL_ERROR, message: "x" });
    await open();
    await tab("ssl");
    expect([...field<HTMLSelectElement>("sslmode")!.options].some((o) => o.disabled)).toBe(false);
  });
});

describe("fields per driver", () => {
  it("sqlite shows the file and the options only", async () => {
    await open();
    await choose(field("driver"), "sqlite");
    const tabs = [...document.querySelectorAll<HTMLElement>("[role=tab]")].map(
      (t) => t.dataset.tab,
    );
    expect(tabs).toEqual(["general", "options"]);
    for (const gone of ["host", "port", "user", "database", "passwordSource"])
      expect(field(gone), gone).toBeNull();
    expect(field("path")).toBeTruthy();
    await type(field("name"), "local");
    await type(field("path"), "data/app.db");
    await submit();
    const [upsert] = calledWith("db.connection.upsert");
    expect(upsert!.params.config).toMatchObject({
      driver: "sqlite",
      path: "data/app.db",
      host: null,
      sslmode: null,
    });
  });
});

describe("the password", () => {
  it("goes only to db.secret.set — not to the saved config, notifications, console or the DOM", async () => {
    await open();
    await type(field("name"), "Prod PG");
    await type(field("host"), "db.internal");
    const input = testid<HTMLInputElement>("db-conn-password")!;
    expect(input.type).toBe("password");
    expect(input.autocomplete).toBe("off");
    await type(input, SECRET);
    expect(document.body.innerHTML).not.toContain(SECRET);
    await tab("options");
    await choose(field("color"), "db.tag.green");
    await type(field("connectTimeout"), "30");
    await submit();

    const upsert = calledWith("db.connection.upsert");
    expect(upsert).toHaveLength(1);
    expect(JSON.stringify(upsert[0]!.params)).not.toContain(SECRET);
    expect(upsert[0]!.params).toMatchObject({
      projectId: "p1",
      worktree: "/work/wt",
      connectionId: "prod-pg",
      location: "shared",
      config: {
        name: "Prod PG",
        host: "db.internal",
        password: { kind: "keychain" },
        sslmode: "verify-full",
        color: "db.tag.green",
        connectTimeout: 30,
      },
    });
    const secrets = calledWith("db.secret.set");
    expect(secrets).toHaveLength(1);
    expect(secrets[0]!.params).toEqual({
      projectId: "p1",
      worktree: "/work/wt",
      connectionId: "prod-pg",
      key: "password",
      value: SECRET,
      target: {
        driver: "postgres",
        host: "db.internal",
        port: 5432,
        user: null,
        tunnel: null,
        sslmode: "verify-full",
        sslrootcert: null,
        sslcert: null,
        sslkey: null,
      },
    });
    const elsewhere = calls.filter((c) => c.method !== "db.secret.set");
    expect(JSON.stringify(elsewhere)).not.toContain(SECRET);
    expect(JSON.stringify(notes)).not.toContain(SECRET);
    expect(notes).toEqual([{ level: "info", title: T.saved("Prod PG") }]);
    expect(consoleLines.join("\n")).not.toContain(SECRET);
    expect(document.body.innerHTML).not.toContain(SECRET);
  });

  it("is not kept when the dialog opens again — only “saved” is shown", async () => {
    const config: DbConnectionConfig = {
      name: "prod",
      driver: "postgres",
      host: "db.internal",
      password: { kind: "keychain" },
    };
    answers["db.connection.list"] = () => list({ connections: [entryOf("prod", config)] });
    await open("prod");
    const input = testid<HTMLInputElement>("db-conn-password")!;
    expect(input.value).toBe("");
    expect(testid("db-conn-password-note")?.textContent).toBe(T.password.stored);
    // 비밀번호를 치지 않고 저장하면 키체인을 건드리지 않는다.
    await submit();
    expect(calledWith("db.connection.upsert")).toHaveLength(1);
    expect(calledWith("db.secret.set")).toHaveLength(0);
  });

  it.each([
    {
      state: { kind: "secretRequired", reason: "missing", target: null, previous: null },
      key: "missing",
    },
    {
      state: { kind: "secretRequired", reason: "targetChanged", target: null, previous: null },
      key: "targetChanged",
    },
    { state: { kind: "approvalRequired", request: { kind: "tunnel" } }, key: "afterApproval" },
  ] as const)("says what is known about the stored password (%#)", async ({ state, key }) => {
    const config: DbConnectionConfig = {
      name: "p",
      driver: "postgres",
      host: "h",
      password: { kind: "keychain" },
    };
    answers["db.connection.list"] = () =>
      list({
        connections: [
          entryOf("p", config, { state: state as unknown as DbConnectionEntry["state"] }),
        ],
      });
    await open("p");
    expect(testid("db-conn-password-note")?.textContent).toBe(T.password[key]);
  });

  it("shows a broken approvals record before asking for the password", async () => {
    const problem = "/data/projects/p1/db.json is unreadable: expected value";
    const config: DbConnectionConfig = {
      name: "p",
      driver: "postgres",
      host: "h",
      password: { kind: "keychain" },
    };
    answers["db.connection.list"] = () =>
      list({
        stateProblem: problem,
        connections: [
          entryOf("p", config, {
            state: {
              kind: "secretRequired",
              reason: "missing",
              target: null as never,
              previous: null,
            },
          }),
        ],
      });
    await open("p");
    const banner = testid("db-conn-state-problem")!;
    expect(banner.textContent).toBe(T.password.stateProblem(problem));
    expect(banner.textContent).toContain("/data/projects/p1/db.json");
    const password = testid("db-conn-password")!;
    expect(
      banner.compareDocumentPosition(password) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it("clears the keychain item when the connection stops using the keychain", async () => {
    const config: DbConnectionConfig = {
      name: "p",
      driver: "postgres",
      host: "h",
      password: { kind: "keychain" },
    };
    answers["db.connection.list"] = () => list({ connections: [entryOf("p", config)] });
    await open("p");
    await choose(field("passwordSource"), "env");
    await type(field("envVariable"), "PGPASSWORD");
    await submit();
    expect(calledWith("db.connection.upsert")[0]!.params.config).toMatchObject({
      password: { kind: "env", variable: "PGPASSWORD" },
    });
    expect(calledWith("db.secret.clear").map((c) => c.params)).toEqual([
      { projectId: "p1", connectionId: "p", key: "password" },
    ]);
  });
});

describe("[Test Connection]", () => {
  it("runs on the unsaved draft: no upsert, no secret.set, the typed password only in that request", async () => {
    await open();
    await type(field("name"), "draft");
    await type(field("host"), "db.internal");
    await type(testid<HTMLInputElement>("db-conn-password"), SECRET);
    await click(testid("db-conn-test"));
    const tests = calledWith("db.connection.test");
    expect(tests).toHaveLength(1);
    expect(tests[0]!.params).toMatchObject({
      projectId: "p1",
      worktree: "/work/wt",
      connectionId: null,
      secret: SECRET,
      draft: { name: "draft", host: "db.internal", sslmode: "verify-full" },
    });
    expect(calls.map((c) => c.method).sort()).toEqual(
      ["db.connection.list", "db.connection.test", "db.driver.list", "db.ssh.hosts"].sort(),
    );
    const result = testid("db-conn-test-result")!;
    expect(result.dataset.result).toBe("ok");
    expect(result.textContent).toContain(T.test.ok("17.6"));
    expect(result.textContent).toContain(T.test.tls("verified"));
    expect(document.body.innerHTML).not.toContain(SECRET);
    expect(consoleLines.join("\n")).not.toContain(SECRET);
    expect(notes).toEqual([]);
  });

  it("tests an edited connection as a draft of that connection and shows an unverified TLS mark", async () => {
    const config: DbConnectionConfig = {
      name: "p",
      driver: "postgres",
      host: "h",
      sslmode: "require",
    };
    answers["db.connection.list"] = () => list({ connections: [entryOf("p", config)] });
    answers["db.connection.test"] = () => ({ version: null, tls: "unverified" });
    await open("p");
    await click(testid("db-conn-test"));
    expect(calledWith("db.connection.test")[0]!.params).toMatchObject({
      connectionId: "p",
      secret: null,
    });
    expect(testid("db-tls-mark")?.dataset.tls).toBe("unverified");
    expect(calledWith("db.connection.upsert")).toHaveLength(0);
  });

  it("explains the host's refusals: password needed, approval needed, driver error", async () => {
    await open();
    await type(field("name"), "x");
    await type(field("host"), "db.internal");
    const failWith = async (error: RpcCallError, text: string) => {
      answers["db.connection.test"] = () => error;
      await click(testid("db-conn-test"));
      const result = testid("db-conn-test-result")!;
      expect(result.dataset.result).toBe("failed");
      expect(result.textContent).toBe(text);
    };
    await failWith(
      new RpcCallError({ code: 3101, message: "needs password", data: {} }),
      T.test.secret,
    );
    await failWith(
      new RpcCallError({ code: 3102, message: "approval", data: { request: { kind: "env" } } }),
      T.test.approval("env"),
    );
    await failWith(
      new RpcCallError({
        code: 3003,
        message: "db",
        data: { error: { kind: "auth", message: "password authentication failed" } },
      }),
      T.test.failed("password authentication failed"),
    );
  });
});

describe("saving", () => {
  it("saves to the personal file when chosen and shows the host's refusal as it is", async () => {
    answers["db.connection.upsert"] = () =>
      new RpcCallError({
        code: ERROR_CODE.INVALID_PARAMS,
        message: "invalid params",
        data: 'ssh_tunnel.host_ref "a;b" must be a host name from ~/.ssh/config',
      });
    await open();
    await type(field("name"), "x");
    await type(field("host"), "db.internal");
    await tab("ssh");
    await click(field("tunnel"));
    await type(field("hostRef"), "a;b");
    await click(
      document.querySelector<HTMLInputElement>(
        '[data-testid=db-conn-location] input[value="local"]',
      ),
    );
    await submit();
    expect(calledWith("db.connection.upsert")[0]!.params).toMatchObject({
      location: "local",
      config: { sshTunnel: { hostRef: "a;b" } },
    });
    expect(testid("db-conn-save-error")?.textContent).toBe(
      T.saveFailed('ssh_tunnel.host_ref "a;b" must be a host name from ~/.ssh/config'),
    );
    expect(calledWith("db.secret.set")).toHaveLength(0);
  });

  it("needs a name before saving", async () => {
    await open();
    await type(field("host"), "db.internal");
    await submit();
    expect(calledWith("db.connection.upsert")).toHaveLength(0);
    expect(document.querySelector('[data-error="name"]')?.textContent).toBe(
      T.field.required(T.field.name),
    );
  });

  it("keeps a saved connection's automatic sslmode and its personal file", async () => {
    const config: DbConnectionConfig = { name: "p", driver: "postgres", host: "localhost" };
    answers["db.connection.list"] = () =>
      list({ connections: [entryOf("p", config, { layers: ["shared", "local"] })] });
    await open("p");
    expect(field("id")!.readOnly).toBe(true);
    expect(
      document.querySelector<HTMLInputElement>(
        '[data-testid=db-conn-location] input[value="local"]',
      )!.checked,
    ).toBe(true);
    await tab("ssl");
    expect(sslmode()).toBe("disable");
    await submit();
    expect(calledWith("db.connection.upsert")[0]!.params).toMatchObject({
      connectionId: "p",
      location: "local",
      config: { sslmode: null },
    });
  });
});

describe("SSH tunnel tab", () => {
  it("picks a host from ~/.ssh/config or takes one typed directly", async () => {
    await open();
    await type(field("name"), "x");
    await type(field("host"), "db.internal");
    await tab("ssh");
    await click(field("tunnel"));
    const pick = field<HTMLSelectElement>("hostPick")!;
    expect([...pick.options].map((o) => o.value)).toEqual(["", "bastion", "jump-eu"]);
    await choose(pick, "jump-eu");
    expect(field("hostRef")!.value).toBe("jump-eu");
    await submit();
    expect(calledWith("db.connection.upsert")[0]!.params.config).toMatchObject({
      sshTunnel: { hostRef: "jump-eu" },
    });
    expect(calledWith("db.ssh.hosts")[0]!.params).toEqual({
      projectId: "p1",
      worktree: "/work/wt",
    });
  });
});

describe("an untrusted project", () => {
  it("does not read ~/.ssh/config and cannot store passwords or test", async () => {
    answers["db.connection.list"] = () => list({ trusted: false });
    await open();
    expect(calledWith("db.ssh.hosts")).toHaveLength(0);
    expect(testid<HTMLInputElement>("db-conn-password")!.disabled).toBe(true);
    expect(testid("db-conn-password-note")?.textContent).toBe(T.password.untrusted);
    expect(testid<HTMLButtonElement>("db-conn-test")!.disabled).toBe(true);
    await tab("ssh");
    await click(field("tunnel"));
    expect(document.body.textContent).toContain(T.ssh.untrusted);
  });
});

describe("actions", () => {
  it("db.connection.add opens a new connection; db.connection.edit needs { connectionId }", async () => {
    const opened: (string | undefined)[] = [];
    const [add, edit] = connectionActions(T.actions, (id) => opened.push(id));
    expect(add!.id).toBe("db.connection.add");
    expect(add!.args).toBeUndefined();
    expect(edit!.id).toBe("db.connection.edit");
    expect(edit!.args).toBe(true);
    void add!.run();
    void edit!.run({ connectionId: "prod" });
    void edit!.run({});
    void edit!.run("prod");
    expect(opened).toEqual([undefined, "prod"]);
    expect(editArgs({ connectionId: "" })).toBeUndefined();
  });
});
