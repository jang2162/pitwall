// @vitest-environment happy-dom
// DB 접근 묻기(access-prompt.tsx, M3-13 — 메인 판정 조건 ①~④): 3101 은 Host 가 준 대상(이전 → 지금)과 까닭을 보이고 비밀번호를
// `db.secret.set` 에만 넣는다(칸은 password · 자동 완성 끔, 값은 화면 · 콘솔 · 오류 글에 남지 않는다). 3102 는 Host 가 준 요청을
// 그대로 보이고 그대로(`previous` 만 빼고) `db.connection.approve` 에 돌려준다. 처음 포커스는 [취소]. 신뢰 거절 · 그 밖의 오류는 묻지
// 않는다. 한 번에 하나만 묻는다.

import { LocalizationProvider } from "@fluent/react";
import {
  type DbApprovalRequest,
  type DbSecretTarget,
  ERROR_CODE,
  RpcCallError,
} from "@pitwall/rpc-client";
import { useEffect } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, vi } from "vitest";

import { bundledLocalization, bundledTranslator } from "../../i18n/testing";
import { act, endTest, it } from "../../test-support/ended-test";
import {
  approvalOf,
  type DbAccessPrompt,
  type DbAccessTarget,
  dbAccessOf,
  useDbAccessPrompt,
} from "./access-prompt";
import { dbAccessStrings } from "./access-strings";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const popup = vi.hoisted(() => ({ calls: [] as boolean[] }));
vi.mock("../../keymap/keymap-provider", () => ({
  useKeymapPopup: (open: boolean) => popup.calls.push(open),
}));

const PASSWORD = "s3cr3t-Pa55-word";

const TARGET: DbSecretTarget = {
  driver: "postgres",
  host: "db.internal",
  port: 5432,
  user: "app",
  tunnel: "bastion",
  sslmode: "verify-full",
  sslrootcert: "/certs/ca.pem",
  sslcert: null,
  sslkey: null,
};

const secretError = (previous: DbSecretTarget | null = null) =>
  new RpcCallError({
    code: 3101,
    message: "db connection `pg` needs its password",
    data: {
      connectionId: "pg",
      key: "password",
      reason: previous ? "targetChanged" : "missing",
      target: TARGET,
      previous,
    } as never,
  });

const approvalError = (request: DbApprovalRequest) =>
  new RpcCallError({
    code: 3102,
    message: "db connection `pg` needs your approval",
    data: { connectionId: "pg", request } as never,
  });

let root: Root | undefined;
let prompt: DbAccessPrompt;
let calls: { method: string; params: Record<string, unknown> }[];
let fail: Error | undefined;
let access: DbAccessTarget;

function Host() {
  const p = useDbAccessPrompt();
  useEffect(() => {
    prompt = p;
  });
  return <>{p.element}</>;
}

beforeEach(async () => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  popup.calls = [];
  calls = [];
  fail = undefined;
  access = {
    calls: {
      call: (async (method: string, params: Record<string, unknown>) => {
        calls.push({ method, params });
        if (fail) throw fail;
        return null;
      }) as never,
    },
    projectId: "p1",
    worktree: "/wt",
    connectionId: "pg",
    connectionName: "billing-prod",
  };
  const element = document.createElement("div");
  document.body.append(element);
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <LocalizationProvider l10n={bundledLocalization("ko")}>
        <Host />
      </LocalizationProvider>,
    ),
  );
});

afterEach(async () => {
  await endTest();
  await act(async () => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const dialog = () => document.querySelector<HTMLElement>("[data-testid=db-access-prompt]");
const button = (action: string) =>
  dialog()!.querySelector<HTMLButtonElement>(`[data-action=${action}]`)!;
const rows = (testId: string) =>
  [...document.querySelectorAll(`[data-testid=${testId}] dd`)].map((d) => d.textContent);

/** 묻기 시작한다. 답은 감싸서 돌려준다 — Promise 를 그대로 돌려주면 `await` 가 답을 기다린다. */
async function ask(error: unknown): Promise<{ answer: Promise<boolean> }> {
  let answer: Promise<boolean> | undefined;
  await act(async () => {
    answer = prompt.resolve(error, access);
  });
  await act(async () => new Promise((r) => setTimeout(r, 0)));
  return { answer: answer! };
}

async function type(value: string) {
  const input = document.querySelector<HTMLInputElement>("[data-testid=db-access-secret]")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

async function click(el: HTMLElement) {
  await act(async () => el.click());
  await act(async () => new Promise((r) => setTimeout(r, 0)));
}

describe("password needed (3101)", () => {
  it("shows the target, starts on Cancel, and sends the password only to db.secret.set", async () => {
    const logs = [
      vi.spyOn(console, "log"),
      vi.spyOn(console, "info"),
      vi.spyOn(console, "warn"),
      vi.spyOn(console, "error"),
      vi.spyOn(console, "debug"),
    ];
    const { answer } = await ask(secretError());
    expect(dialog()?.dataset.kind).toBe("secret");
    expect(dialog()!.textContent).toContain("billing-prod 의 비밀번호");
    expect(dialog()!.textContent).toContain("저장한 비밀번호가 없다");
    expect(rows("db-access-now")).toEqual([
      "postgres",
      "db.internal",
      "5432",
      "app",
      "bastion",
      "verify-full",
      "/certs/ca.pem",
    ]);
    expect(document.activeElement).toBe(button("cancel"));
    expect(popup.calls).toContain(true);
    const input = document.querySelector<HTMLInputElement>("[data-testid=db-access-secret]")!;
    expect(input.type).toBe("password");
    expect(input.getAttribute("autocomplete")).toBe("off");

    await type(PASSWORD);
    await click(button("run"));
    expect(await answer).toBe(true);
    expect(calls).toEqual([
      {
        method: "db.secret.set",
        params: {
          projectId: "p1",
          worktree: "/wt",
          connectionId: "pg",
          key: "password",
          target: TARGET,
          value: PASSWORD,
        },
      },
    ]);
    expect(dialog()).toBeNull();
    expect(document.documentElement.outerHTML).not.toContain(PASSWORD);
    for (const log of logs)
      for (const args of log.mock.calls) expect(JSON.stringify(args)).not.toContain(PASSWORD);
  });

  it("shows the target the password was saved for next to the one now", async () => {
    const previous = { ...TARGET, host: "db.old", tunnel: null };
    void (await ask(secretError(previous)));
    expect(dialog()!.textContent).toContain("비밀번호를 저장할 때와 연결 대상이 다르다");
    expect(rows("db-access-previous")).toContain("db.old");
    expect(rows("db-access-previous")).toContain("(없음)");
    expect(rows("db-access-now")).toContain("db.internal");
    await click(button("cancel"));
  });

  it("sends nothing when cancelled, and keeps the dialog with the error when the host refuses", async () => {
    const { answer: cancelled } = await ask(secretError());
    await type(PASSWORD);
    await click(button("cancel"));
    expect(await cancelled).toBe(false);
    expect(calls).toEqual([]);

    const { answer } = await ask(secretError());
    fail = new RpcCallError({ code: ERROR_CODE.CONFLICT, message: "the target changed" });
    await type(PASSWORD);
    await click(button("run"));
    expect(document.querySelector("[data-testid=db-access-failed]")?.textContent).toBe(
      `하지 못했다: the target changed (${ERROR_CODE.CONFLICT})`,
    );
    expect(dialog()).not.toBeNull();
    fail = undefined;
    await click(button("run"));
    expect(await answer).toBe(true);
  });
});

describe("approval needed (3102)", () => {
  const cases: [string, DbApprovalRequest, string[]][] = [
    [
      "env",
      {
        kind: "env",
        variable: "PGPASSWORD_PROD",
        target: TARGET,
        previous: { variable: "PGPASSWORD", target: TARGET },
      },
      ["PGPASSWORD_PROD", "postgres", "db.internal"],
    ],
    [
      "file",
      { kind: "file", realPath: "/Users/me/elsewhere/app.db" },
      ["/Users/me/elsewhere/app.db"],
    ],
    [
      "tunnel",
      { kind: "tunnel", hostRef: "ssh://ops@bastion:2222", target: TARGET, previous: null },
      ["ssh://ops@bastion:2222", "db.internal"],
    ],
    [
      "certFiles",
      {
        kind: "certFiles",
        files: { sslrootcert: "/r/ca.pem", sslcert: "/r/client.pem", sslkey: "/r/client.key" },
        target: TARGET,
        previous: null,
      },
      ["/r/ca.pem", "/r/client.pem", "/r/client.key"],
    ],
  ];

  for (const [kind, request, shown] of cases)
    it(`shows ${kind} as the host sent it and approves it unchanged`, async () => {
      const { answer } = await ask(approvalError(request));
      expect(dialog()?.dataset.kind).toBe(kind);
      expect(document.activeElement).toBe(button("cancel"));
      for (const value of shown) expect(rows("db-access-now")).toContain(value);
      await click(button("run"));
      expect(await answer).toBe(true);
      const { previous: _previous, ...expected } = request as DbApprovalRequest & {
        previous?: unknown;
      };
      expect(calls).toEqual([
        {
          method: "db.connection.approve",
          params: { projectId: "p1", worktree: "/wt", connectionId: "pg", approval: expected },
        },
      ]);
      expect(approvalOf(request)).toEqual(expected);
    });

  it("shows what was approved before", async () => {
    void (await ask(approvalError(cases[0]![1])));
    expect(rows("db-access-previous")).toContain("PGPASSWORD");
    await click(button("cancel"));
  });
});

it("does not ask for a trust refusal or any other error, and asks one thing at a time", async () => {
  const untrusted = new RpcCallError({
    code: ERROR_CODE.PERMISSION_DENIED,
    message: "untrusted",
    data: { reason: "untrusted" } as never,
  });
  const dbError = new RpcCallError({
    code: 3003,
    message: "db error",
    data: { error: { kind: "connect", message: "refused", code: null } } as never,
  });
  for (const error of [untrusted, dbError, new Error("boom"), undefined]) {
    expect(await (await ask(error)).answer).toBe(false);
    expect(dialog()).toBeNull();
  }
  expect(dbAccessOf(untrusted)).toBeUndefined();

  const { answer: first } = await ask(secretError());
  expect(await (await ask(approvalError(cases0()))).answer).toBe(false);
  expect(dialog()?.dataset.kind).toBe("secret");
  await click(button("cancel"));
  expect(await first).toBe(false);
  expect(calls).toEqual([]);
});

function cases0(): DbApprovalRequest {
  return { kind: "file", realPath: "/x.db" };
}

it("has every string in both languages", async () => {
  for (const locale of ["ko", "en"]) {
    const T = dbAccessStrings(bundledTranslator(locale));
    expect(T.secret.title("x")).toContain("x");
    expect(Object.values(T.approve.title).every((s) => s.length > 0)).toBe(true);
  }
});
