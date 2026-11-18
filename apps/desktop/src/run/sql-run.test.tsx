// @vitest-environment happy-dom
// 실행 구성의 `sql` 종류를 창에서 돌릴 때(17-m6-plan M6-09 ③): Host 의 `3001`(`RunSqlConfirmRequired`)을 SQL 콘솔과 같은 위험
// 확인 다이얼로그로 묻고, 받은 확인을 `confirmed` 로 실어 다시 부른다. 파일이 그사이 바뀌어 다시 `3001` 이 오면 새 내용으로 다시
// 묻는다. 취소하면 다시 부르지 않는다. 창의 키맵은 가짜로 둔다.

import { LocalizationProvider } from "@fluent/react";
import {
  type RpcError,
  type RunInfo,
  type RunSqlConfirmRequired,
  type RunSqlStartParams,
  RpcCallError,
} from "@pitwall/rpc-client";
import { useEffect, useRef } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, vi } from "vitest";

import { act, endTest, it } from "../test-support/ended-test";
import { bundledLocalization } from "../i18n/testing";
import { type RiskConfirmQuestion, useRiskConfirm } from "../db/safety/risk-confirm";
import { type SqlRunStarted, sqlRunConfirmOf, sqlRunQuestion, startSqlRun } from "./sql-run";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

vi.mock("../keymap/keymap-provider", () => ({ useKeymapPopup: () => undefined }));

let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
});

afterEach(async () => {
  await endTest();
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
});

const SQL = "INSERT INTO a VALUES (1);\nDROP TABLE t;\n";

function asked(over: Partial<RunSqlConfirmRequired["request"]> = {}): RunSqlConfirmRequired {
  return {
    request: {
      grade: "destructive",
      statements: [
        { index: 0, range: { from: 0, to: 24 }, grade: "write", reasons: ["dml"] },
        { index: 1, range: { from: 26, to: 39 }, grade: "destructive", reasons: ["drop"] },
      ],
      statementsHash: "h-1",
      effects: [],
      nameRequired: true,
      rejected: null,
      ...over,
    },
    connection: { id: "prod", name: "Prod", production: true, readOnly: false },
    files: [
      { path: "db/seed.sql", contentHash: "aa", statements: 1 },
      { path: "db/wipe.sql", contentHash: "bb", statements: 1 },
    ],
    transaction: "per_file",
    sql: SQL,
  };
}

const confirmError = (data: RunSqlConfirmRequired) =>
  new RpcCallError({ code: 3001, message: "confirm", data } as unknown as RpcError);

const RUN = { runId: "r-1", kind: "sql" } as unknown as RunInfo;

it("a 3001 from run.sql.start becomes the SQL console's question", async () => {
  const data = asked();
  expect(sqlRunConfirmOf(confirmError(data))).toEqual(data);
  expect(sqlRunConfirmOf(new RpcCallError({ code: -32602, message: "x" } as RpcError))).toBe(
    undefined,
  );
  const question = sqlRunQuestion(data);
  expect(question.request).toBe(data.request);
  expect(question.sql).toBe(SQL);
  expect(question.connection).toEqual({ name: "Prod", production: true, readOnly: false });
});

it("asks, then calls again with the person's confirmation", async () => {
  const calls: RunSqlStartParams[] = [];
  const call = async (params: RunSqlStartParams) => {
    calls.push(params);
    if (calls.length === 1) throw confirmError(asked());
    return RUN;
  };
  const questions: RiskConfirmQuestion[] = [];
  const started = await startSqlRun(call, { projectId: "p", configId: "seed" }, async (q) => {
    questions.push(q);
    return { grade: q.request.grade, statementsHash: q.request.statementsHash, name: "Prod" };
  });
  expect(started).toEqual({ kind: "started", run: RUN });
  expect(questions).toHaveLength(1);
  expect(calls[0]?.confirmed).toBe(null);
  expect(calls[1]?.confirmed).toEqual({
    grade: "destructive",
    statementsHash: "h-1",
    name: "Prod",
  });
});

it("a file changed after the confirmation asks again with the new contents", async () => {
  let n = 0;
  const call = async () => {
    n += 1;
    if (n === 1) throw confirmError(asked());
    if (n === 2) throw confirmError(asked({ statementsHash: "h-2", rejected: "hashMismatch" }));
    return RUN;
  };
  const hashes: string[] = [];
  const started = await startSqlRun(call, { projectId: "p", configId: "seed" }, async (q) => {
    hashes.push(q.request.statementsHash);
    return { grade: q.request.grade, statementsHash: q.request.statementsHash, name: "Prod" };
  });
  expect(started.kind).toBe("started");
  expect(hashes).toEqual(["h-1", "h-2"]);
});

it("cancelling the question does not run it", async () => {
  let n = 0;
  const call = async () => {
    n += 1;
    throw confirmError(asked());
  };
  const started = await startSqlRun(
    call,
    { projectId: "p", configId: "seed" },
    async () => undefined,
  );
  expect(started).toEqual({ kind: "cancelled" });
  expect(n).toBe(1);
});

it("other refusals are thrown as they are", async () => {
  const refusal = new RpcCallError({
    code: -32002,
    message: "permission denied",
    data: { reason: "readOnlyConnection" },
  } as unknown as RpcError);
  await expect(
    startSqlRun(
      async () => {
        throw refusal;
      },
      { projectId: "p", configId: "seed" },
      async () => undefined,
    ),
  ).rejects.toBe(refusal);
});

it("the question opens the SQL console's risk dialog, with the connection name to type", async () => {
  const results: SqlRunStarted[] = [];
  function Harness() {
    const risk = useRiskConfirm();
    const once = useRef(false);
    useEffect(() => {
      // 시험은 StrictMode 로 그린다 — 실행은 한 번만 시작한다(사람의 클릭 한 번).
      if (once.current) return;
      once.current = true;
      let n = 0;
      void startSqlRun(
        async () => {
          n += 1;
          if (n === 1) throw confirmError(asked());
          return RUN;
        },
        { projectId: "p", configId: "seed" },
        risk.confirm,
      ).then((r) => results.push(r));
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);
    return <>{risk.element}</>;
  }
  const element = document.createElement("div");
  document.body.append(element);
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <LocalizationProvider l10n={bundledLocalization("ko")}>
        <Harness />
      </LocalizationProvider>,
    ),
  );
  const dialog = document.querySelector<HTMLElement>("[data-testid=db-risk-confirm]");
  expect(dialog).not.toBeNull();
  expect(dialog!.dataset.grade).toBe("destructive");
  const statements = [
    ...document.querySelectorAll<HTMLElement>("[data-testid=db-confirm-statement]"),
  ];
  expect(statements.map((s) => s.dataset.grade)).toEqual(["write", "destructive"]);
  expect(dialog!.textContent).toContain("DROP TABLE t");
  const name = document.querySelector<HTMLInputElement>("[data-testid=db-confirm-name]")!;
  const run = document.querySelector<HTMLButtonElement>("[data-action=run]")!;
  expect(run.disabled).toBe(true);
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
  await act(async () => {
    setter.call(name, "Prod");
    name.dispatchEvent(new Event("input", { bubbles: true }));
  });
  expect(run.disabled).toBe(false);
  await act(async () => run.click());
  expect(results).toEqual([{ kind: "started", run: RUN }]);
});
