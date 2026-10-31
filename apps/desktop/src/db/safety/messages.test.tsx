// @vitest-environment happy-dom
// 거절 문장 · `3001` 꺼내기 · 세션 알림 문구(M3-19 ③, M3-05 ⓒ ⓓ).

import { LocalizationProvider } from "@fluent/react";
import { type DbSessionNotice, ERROR_CODE, RpcCallError } from "@pitwall/rpc-client";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect } from "vitest";

import { act, endTest, it } from "../../test-support/ended-test";
import { bundledLocalization, bundledTranslator } from "../../i18n/testing";
import {
  confirmRequiredOf,
  DB_ERROR_CODE,
  dbNoticeText,
  dbRefusal,
  dbRefusalText,
  DbRefusalMessage,
} from "./messages";
import { dbSafetyStrings } from "./strings";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const T = dbSafetyStrings(bundledTranslator("ko"));
const EN = dbSafetyStrings(bundledTranslator("en"));
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

const err = (code: number, data?: object) =>
  new RpcCallError({ code, message: "m", data: data as never });

describe("dbRefusal", () => {
  it("reads the read-only and untrusted refusals with their sentences", async () => {
    const statements = [{ index: 0, range: { from: 0, to: 8 }, grade: "write", reasons: ["dml"] }];
    const readOnly = dbRefusal(
      err(ERROR_CODE.PERMISSION_DENIED, { reason: "readOnlyConnection", statements }),
    );
    expect(readOnly).toEqual({ reason: "readOnlyConnection", message: "", statements });
    expect(dbRefusalText(readOnly!, T.refusal)).toBe(
      "읽기 전용 연결이라 조회만 돌린다 — 이 문장은 돌리지 않았다",
    );
    const untrusted = dbRefusal(
      err(ERROR_CODE.PERMISSION_DENIED, { reason: "untrusted", projectId: "p1" }),
    );
    expect(dbRefusalText(untrusted!, T.refusal)).toBe(
      "이 프로젝트를 신뢰하지 않아 DB 에 연결하지 않는다 — 프로젝트를 신뢰한 뒤 다시 연결한다",
    );
    const statement = dbRefusal(
      err(ERROR_CODE.INVALID_PARAMS, {
        reason: "transactionStatement",
        index: 0,
        message: "PREPARE TRANSACTION",
      }),
    );
    expect(dbRefusalText(statement!, T.refusal)).toContain("PREPARE TRANSACTION");
  });

  it("shows the not-a-query refusal of db.query.page as a refusal sentence", async () => {
    // Host 가 내는 그대로(crates/pw-db/src/execute.rs `only_one_query`): PERMISSION_DENIED · notAQuery · 거절한 문장.
    const statements = [
      { index: 1, range: { from: 21, to: 33 }, grade: "destructive", reasons: ["drop"] },
    ];
    const host = err(ERROR_CODE.PERMISSION_DENIED, { reason: "notAQuery", statements });
    expect(dbRefusal(host)).toEqual({ reason: "notAQuery", message: "", statements });
    // 같은 이름이라도 다른 코드면 거절로 읽지 않는다.
    expect(dbRefusal(err(ERROR_CODE.INVALID_PARAMS, { reason: "notAQuery" }))).toBeUndefined();
    const element = document.createElement("div");
    document.body.append(element);
    root = createRoot(element);
    await act(async () =>
      root!.render(
        <LocalizationProvider l10n={bundledLocalization("ko")}>
          <DbRefusalMessage error={host} />
        </LocalizationProvider>,
      ),
    );
    const alert = element.querySelector<HTMLElement>("[role=alert]")!;
    expect(alert.dataset.reason).toBe("notAQuery");
    expect(alert.textContent).toBe(T.refusal.notAQuery());
  });

  it("does not read other errors as refusals", async () => {
    expect(dbRefusal(new Error("x"))).toBeUndefined();
    expect(dbRefusal(err(ERROR_CODE.PERMISSION_DENIED))).toBeUndefined();
    expect(dbRefusal(err(ERROR_CODE.PERMISSION_DENIED, { reason: "toString" }))).toBeUndefined();
    // 같은 까닭 이름이라도 코드가 다르면 아니다(`CONFLICT` 의 `busy` 를 `PERMISSION_DENIED` 로 받지 않는다).
    expect(dbRefusal(err(ERROR_CODE.PERMISSION_DENIED, { reason: "busy" }))).toBeUndefined();
    expect(dbRefusal(err(ERROR_CODE.CONFLICT, { reason: "untrusted" }))).toBeUndefined();
    expect(dbRefusal(err(ERROR_CODE.CONFLICT, { reason: "busy" }))?.reason).toBe("busy");
  });

  it("words every refusal distinctly in both languages", async () => {
    for (const strings of [T, EN]) {
      const texts = Object.values(strings.refusal).map((f) => f("why"));
      expect(texts).toHaveLength(8);
      expect(new Set(texts).size).toBe(texts.length);
    }
  });

  it("renders the refusal as an alert and nothing for other errors", async () => {
    const element = document.createElement("div");
    document.body.append(element);
    root = createRoot(element);
    const render = (error: unknown) =>
      act(async () =>
        root!.render(
          <LocalizationProvider l10n={bundledLocalization("ko")}>
            <DbRefusalMessage error={error} />
          </LocalizationProvider>,
        ),
      );
    await render(err(ERROR_CODE.PERMISSION_DENIED, { reason: "untrusted" }));
    const alert = element.querySelector<HTMLElement>("[role=alert]")!;
    expect(alert.dataset.reason).toBe("untrusted");
    expect(alert.textContent).toBe(T.refusal.untrusted());
    await render(new Error("boom"));
    expect(element.innerHTML).toBe("");
  });
});

describe("confirmRequiredOf", () => {
  it("takes the data of 3001 and nothing else", async () => {
    const data = {
      grade: "destructive",
      statements: [],
      statementsHash: "abc",
      effects: [],
      nameRequired: true,
      rejected: null,
    };
    expect(confirmRequiredOf(err(DB_ERROR_CODE.CONFIRM_REQUIRED, data))).toEqual(data);
    expect(confirmRequiredOf(err(DB_ERROR_CODE.DB_ERROR, data))).toBeUndefined();
    expect(confirmRequiredOf(err(DB_ERROR_CODE.CONFIRM_REQUIRED, { grade: "read" }))).toBe(
      undefined,
    );
    expect(confirmRequiredOf(new Error("x"))).toBeUndefined();
  });
});

describe("dbNoticeText", () => {
  const notices: DbSessionNotice[] = [
    { kind: "rolledBack", message: "deadlock detected" },
    { kind: "committedImplicitly" },
    { kind: "transactionEnded", message: null },
    { kind: "transactionEnded", message: "PROCEDURE failed" },
    { kind: "transactionUnknown", message: "ping failed" },
    { kind: "transactionOpened" },
    { kind: "connectionLost", hadTransaction: false, outcomeUnknown: false },
    { kind: "connectionLost", hadTransaction: true, outcomeUnknown: false },
    { kind: "connectionLost", hadTransaction: true, outcomeUnknown: true },
    { kind: "configChanged" },
    { kind: "unattended", until: 1_700_000_000_000 },
    { kind: "unattendedRolledBack" },
  ];

  it("words every session notice, each case differently", async () => {
    expect(new Set(notices.map((n) => n.kind))).toEqual(
      new Set([
        "rolledBack",
        "committedImplicitly",
        "transactionEnded",
        "transactionUnknown",
        "transactionOpened",
        "connectionLost",
        "configChanged",
        "unattended",
        "unattendedRolledBack",
      ]),
    );
    for (const strings of [T, EN]) {
      const texts = notices.map((n) => dbNoticeText(n, strings.notice, () => "10:30"));
      expect(new Set(texts).size).toBe(texts.length);
    }
  });

  it("says plainly when the outcome is unknown", async () => {
    const t = (n: DbSessionNotice) => dbNoticeText(n, T.notice, () => "10:30");
    expect(t({ kind: "transactionEnded", message: null })).toBe(
      "트랜잭션이 끝났다. 커밋됐는지 롤백됐는지 서버가 알리지 않았다 — 다른 연결로 확인하라",
    );
    expect(t({ kind: "transactionEnded", message: "PROCEDURE failed" })).toContain(
      "PROCEDURE failed",
    );
    expect(t({ kind: "connectionLost", hadTransaction: true, outcomeUnknown: true })).toBe(
      "커밋 도중 끊겨 결과를 모른다 — 다른 연결로 확인하라",
    );
    expect(t({ kind: "rolledBack", message: "deadlock detected" })).toContain("deadlock detected");
    expect(t({ kind: "transactionUnknown", message: "ping failed" })).toContain("ping failed");
    expect(t({ kind: "unattended", until: 1 })).toContain("10:30");
  });
});
