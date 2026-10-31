// @vitest-environment happy-dom
// 위험 쿼리 확인 다이얼로그(M3-19 ②): 등급마다 문구 · 처음 포커스([취소]), 프로덕션 파괴 등급의 연결 이름 입력(다르면 [실행] 이
// 꺼짐 · 같으면 친 이름과 받은 해시를 돌려줌), 한글 조합 중의 Enter · Esc, `unparsed` 문장의 「분석하지 못한 문장」 과 읽지 못한
// 파괴의 제목, 트랜잭션 효과 · 다시 묻는 까닭, 한 번에 하나만 묻기(같은 요청 · 다른 요청 · 주인 언마운트), 실행 전 정책
// (`confirmationFor`). 창의 키맵은 가짜로 둔다 — 떠 있는 동안 키맵을 쉬게 하는지만 본다.

import { LocalizationProvider } from "@fluent/react";
import type {
  DbConfirmed,
  DbConfirmRequired,
  DbKvConfirmRequired,
  DbSqlClassification,
  DbStatementClass,
  RiskGrade,
  RiskReason,
} from "@pitwall/rpc-client";
import { useEffect } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, vi } from "vitest";

import { act, endTest, it } from "../../test-support/ended-test";
import { bundledLocalization, bundledTranslator } from "../../i18n/testing";
import {
  type ConfirmQuestion,
  confirmationFor,
  KV_ARGS_SHOWN,
  type RiskConfirmAnswer,
  RiskConfirmDialog,
  STATEMENTS_SHOWN,
  useRiskConfirm,
} from "./risk-confirm";
import { dbSafetyStrings } from "./strings";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const popup = vi.hoisted(() => ({ calls: [] as boolean[] }));
vi.mock("../../keymap/keymap-provider", () => ({
  useKeymapPopup: (open: boolean) => popup.calls.push(open),
}));

const T = dbSafetyStrings(bundledTranslator("ko"));
const EN = dbSafetyStrings(bundledTranslator("en"));

let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  popup.calls = [];
});

afterEach(async () => {
  await endTest();
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
});

function statement(
  index: number,
  grade: RiskGrade,
  reasons: RiskReason[],
  range = { from: 0, to: 0 },
): DbStatementClass {
  return { index, range, grade, reasons };
}

function request(over: Partial<DbConfirmRequired> = {}): DbConfirmRequired {
  return {
    grade: "unboundedWrite",
    statements: [statement(0, "unboundedWrite", ["missingWhere"], { from: 0, to: 13 })],
    statementsHash: "h-1",
    effects: [],
    nameRequired: false,
    rejected: null,
    ...over,
  };
}

const DEV = { name: "local-pg", production: false, readOnly: false, color: "db.tag.green" };
const PROD = { name: "prod-billing", production: true, readOnly: false, color: null };

async function show(question: ConfirmQuestion) {
  const answers: (DbConfirmed | undefined)[] = [];
  const element = document.createElement("div");
  document.body.append(element);
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <LocalizationProvider l10n={bundledLocalization("ko")}>
        <RiskConfirmDialog question={question} onAnswer={(a) => answers.push(a)} />
      </LocalizationProvider>,
    ),
  );
  return answers;
}

const dialog = () => document.querySelector<HTMLElement>("[data-testid=db-risk-confirm]")!;
const button = (action: "cancel" | "run") =>
  document.querySelector<HTMLButtonElement>(`[data-action=${action}]`)!;
const nameInput = () => document.querySelector<HTMLInputElement>("[data-testid=db-confirm-name]");

async function type(input: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
  await act(async () => {
    setter.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

describe("RiskConfirmDialog", () => {
  it.each([
    { grade: "write", reasons: ["dml"] },
    { grade: "unboundedWrite", reasons: ["missingWhere"] },
    { grade: "ddl", reasons: ["ddl"] },
    { grade: "destructive", reasons: ["drop"] },
  ] as const)(
    "words the $grade grade, lists the reasons and focuses [Cancel] first",
    async ({ grade, reasons }) => {
      await show({
        request: request({ grade, statements: [statement(0, grade, [...reasons])] }),
        connection: DEV,
      });
      expect(dialog().dataset.grade).toBe(grade);
      expect(dialog().textContent).toContain(T.confirm.title[grade]);
      expect(dialog().textContent).toContain(T.confirm.grade(T.grade[grade]));
      for (const reason of reasons) expect(dialog().textContent).toContain(T.reason[reason]);
      expect(document.activeElement).toBe(button("cancel"));
      expect(button("run").disabled).toBe(false);
      // 떠 있는 동안 창의 키맵은 쉰다.
      expect(popup.calls).toContain(true);
    },
  );

  it("titles differ for every grade", async () => {
    expect(new Set([...Object.values(T.confirm.title), T.confirm.titleUnread]).size).toBe(6);
    expect(new Set(Object.values(T.grade)).size).toBe(5);
  });

  it("returns the received hash and grade when run, and nothing when cancelled or dismissed", async () => {
    const answers = await show({ request: request(), connection: DEV });
    await act(async () => button("run").click());
    expect(answers).toEqual([{ grade: "unboundedWrite", statementsHash: "h-1", name: null }]);

    act(() => root?.unmount());
    const cancelled = await show({ request: request(), connection: DEV });
    await act(async () => button("cancel").click());
    expect(cancelled).toEqual([undefined]);

    act(() => root?.unmount());
    const escaped = await show({ request: request(), connection: DEV });
    await act(async () =>
      dialog().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })),
    );
    expect(escaped).toEqual([undefined]);
  });

  it("keeps [Run] off on a production destructive run until the connection name is typed exactly", async () => {
    const answers = await show({
      request: request({
        grade: "destructive",
        statements: [statement(0, "destructive", ["drop"])],
        nameRequired: true,
      }),
      connection: PROD,
    });
    expect(dialog().textContent).toContain(T.confirm.production("prod-billing"));
    expect(dialog().textContent).toContain(T.confirm.namePrompt("prod-billing"));
    // 이름 칸이 있어도 처음 포커스는 [취소]다.
    expect(document.activeElement).toBe(button("cancel"));
    expect(button("run").disabled).toBe(true);

    for (const wrong of ["prod-billin", "prod-billing ", "PROD-BILLING", "local-pg"]) {
      await type(nameInput()!, wrong);
      expect(button("run").disabled, wrong).toBe(true);
      expect(nameInput()!.getAttribute("aria-invalid")).toBe("true");
      // 틀린 이름의 Enter 는 아무것도 보내지 않는다.
      await act(async () =>
        nameInput()!.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })),
      );
      expect(answers).toEqual([]);
    }

    await type(nameInput()!, "prod-billing");
    expect(button("run").disabled).toBe(false);
    await act(async () => button("run").click());
    expect(answers).toEqual([
      { grade: "destructive", statementsHash: "h-1", name: "prod-billing" },
    ]);
  });

  it("lets a Korean IME finish composing the name: Enter and Esc while composing neither run nor close", async () => {
    const answers = await show({
      request: request({
        grade: "destructive",
        statements: [statement(0, "destructive", ["drop"])],
        nameRequired: true,
      }),
      connection: { ...PROD, name: "결제-운영" },
    });
    await type(nameInput()!, "결제-운영");
    expect(button("run").disabled).toBe(false);
    // 조합을 끝내는 Enter(조합 중 · WebKit 의 keyCode 229)는 실행하지 않는다.
    for (const init of [{ isComposing: true }, { keyCode: 229 }])
      await act(async () =>
        nameInput()!.dispatchEvent(
          new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true, ...init }),
        ),
      );
    expect(answers).toEqual([]);
    // 조합을 취소하는 Esc 는 다이얼로그를 닫지 않는다 — 친 이름이 남는다.
    for (const init of [{ isComposing: true }, { keyCode: 229 }])
      await act(async () =>
        nameInput()!.dispatchEvent(
          new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true, ...init }),
        ),
      );
    expect(answers).toEqual([]);
    expect(dialog()).not.toBeNull();
    expect(nameInput()!.value).toBe("결제-운영");
    // 조합이 끝난 뒤의 Enter 는 실행한다.
    await act(async () =>
      nameInput()!.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })),
    );
    expect(answers).toEqual([{ grade: "destructive", statementsHash: "h-1", name: "결제-운영" }]);
  });

  it("does not ask for the name when the Host does not require it", async () => {
    const answers = await show({
      request: request({
        grade: "destructive",
        statements: [statement(0, "destructive", ["drop"])],
      }),
      connection: DEV,
    });
    expect(nameInput()).toBeNull();
    await act(async () => button("run").click());
    expect(answers).toEqual([{ grade: "destructive", statementsHash: "h-1", name: null }]);
  });

  it("marks a statement that could not be analyzed and shows each statement's text, grade and reasons", async () => {
    const sql = "DELETE FROM t;\nFROBNICATE everything;";
    await show({
      request: request({
        grade: "destructive",
        statements: [
          statement(0, "unboundedWrite", ["missingWhere"], { from: 0, to: 13 }),
          statement(1, "destructive", ["unparsed"], { from: 15, to: 36 }),
        ],
      }),
      connection: DEV,
      sql,
    });
    const items = [...document.querySelectorAll<HTMLElement>("[data-testid=db-confirm-statement]")];
    expect(items.map((i) => i.dataset.grade)).toEqual(["unboundedWrite", "destructive"]);
    expect(items[0]!.textContent).toContain("DELETE FROM t");
    expect(items[0]!.textContent).toContain(T.reason.missingWhere);
    expect(items[0]!.querySelector("[data-testid=db-confirm-unparsed]")).toBeNull();
    expect(items[1]!.dataset.unparsed).toBe("true");
    expect(items[1]!.textContent).toContain("FROBNICATE everything");
    expect(items[1]!.querySelector("[data-testid=db-confirm-unparsed]")!.textContent).toBe(
      "분석하지 못한 문장",
    );
    expect(items[1]!.textContent).toContain(T.reason.unparsed);
    expect(dialog().textContent).toContain(T.confirm.statements(2));
  });

  it.each([
    { reasons: [["unparsed"]], title: "unread" },
    { reasons: [["tooLarge"], ["tooDeep"]], title: "unread" },
    { reasons: [["unparsed"], ["drop"]], title: "destructive" },
    { reasons: [["truncate"]], title: "destructive" },
  ] as const)(
    "titles a destructive run by what is known: $title for $reasons",
    async ({ reasons, title }) => {
      await show({
        request: request({
          grade: "destructive",
          statements: reasons.map((r, i) => statement(i, "destructive", [...r])),
        }),
        connection: DEV,
      });
      const heading = dialog().querySelector<HTMLElement>("[data-title]")!;
      expect(heading.dataset.title).toBe(title);
      expect(heading.textContent).toBe(
        title === "unread" ? T.confirm.titleUnread : T.confirm.title.destructive,
      );
    },
  );

  it("shows the implicit commit effect and why the Host asks again", async () => {
    await show({
      request: request({ grade: "ddl", effects: ["implicitCommit"], rejected: "hashMismatch" }),
      connection: DEV,
    });
    expect(document.querySelector("[data-testid=db-confirm-effect]")!.textContent).toBe(
      "이 문장은 열린 트랜잭션을 먼저 커밋한다 — 실패해도 앞의 변경은 커밋된다",
    );
    expect(document.querySelector("[data-testid=db-confirm-rejected]")!.textContent).toBe(
      T.confirm.rejected.hashMismatch,
    );
    act(() => root?.unmount());
    await show({ request: request({ rejected: "somethingNew" }), connection: DEV });
    expect(document.querySelector("[data-testid=db-confirm-rejected]")!.textContent).toBe(
      T.confirm.rejected.other,
    );
  });

  it("lists at most the first statements and counts the rest", async () => {
    const many = Array.from({ length: STATEMENTS_SHOWN + 7 }, (_, i) =>
      statement(i, "write", ["dml"]),
    );
    await show({ request: request({ grade: "write", statements: many }), connection: PROD });
    expect(document.querySelectorAll("[data-testid=db-confirm-statement]")).toHaveLength(
      STATEMENTS_SHOWN,
    );
    expect(dialog().textContent).toContain(T.confirm.more(7));
  });

  it("shows the connection name with its labels, not only its color", async () => {
    await show({ request: request(), connection: { ...PROD, readOnly: true } });
    const mark = document.querySelector<HTMLElement>("[data-testid=db-connection-mark]")!;
    expect(mark.textContent).toContain("prod-billing");
    expect(mark.textContent).toContain(T.label.production);
    expect(mark.textContent).toContain(T.label.readOnly);
  });
});

describe("useRiskConfirm", () => {
  let confirm: ReturnType<typeof useRiskConfirm>["confirm"] | undefined;
  function Asker() {
    const asked = useRiskConfirm();
    useEffect(() => {
      confirm = asked.confirm;
    });
    return <>{asked.element}</>;
  }
  async function mountAsker() {
    confirm = undefined;
    const element = document.createElement("div");
    document.body.append(element);
    root = createRoot(element);
    await act(async () =>
      root!.render(
        <LocalizationProvider l10n={bundledLocalization("ko")}>
          <Asker />
        </LocalizationProvider>,
      ),
    );
  }

  it("asks one question at a time — the same request shares the answer, another one is told busy", async () => {
    await mountAsker();
    let first: Promise<RiskConfirmAnswer> | undefined;
    let again: Promise<RiskConfirmAnswer> | undefined;
    await act(async () => {
      first = confirm!({ request: request(), connection: DEV });
    });
    // 실행 키를 두 번 눌러 같은 요청을 다시 묻는다 — 다이얼로그는 하나, 답은 같다.
    await act(async () => {
      again = confirm!({ request: request(), connection: DEV });
    });
    expect(document.querySelectorAll("[data-testid=db-risk-confirm]")).toHaveLength(1);
    // 다른 요청은 묻지 않는다 — 사람의 취소(`undefined`)와 구별된다.
    await expect(
      confirm!({ request: request({ statementsHash: "h-2" }), connection: DEV }),
    ).resolves.toBe("busy");
    await expect(
      confirm!({ request: request(), connection: { ...DEV, name: "other-pg" } }),
    ).resolves.toBe("busy");
    await act(async () => button("run").click());
    const confirmed = { grade: "unboundedWrite", statementsHash: "h-1", name: null };
    await expect(first).resolves.toEqual(confirmed);
    await expect(again).resolves.toEqual(confirmed);
    expect(dialog()).toBeNull();

    // 답한 뒤에는 다시 묻는다.
    let next: Promise<RiskConfirmAnswer> | undefined;
    await act(async () => {
      next = confirm!({ request: request({ statementsHash: "h-2" }), connection: DEV });
    });
    await act(async () => button("cancel").click());
    await expect(next).resolves.toBeUndefined();
  });

  it("ends a pending question as cancelled when its owner unmounts", async () => {
    await mountAsker();
    let pending: Promise<RiskConfirmAnswer> | undefined;
    await act(async () => {
      pending = confirm!({ request: request(), connection: DEV });
    });
    expect(dialog()).not.toBeNull();
    act(() => root!.unmount());
    root = undefined;
    const settled = await Promise.race([
      pending!.then((value) => ({ value })),
      new Promise((r) => setTimeout(() => r("still waiting"), 50)),
    ]);
    expect(settled).toEqual({ value: undefined });
  });
});

describe("confirmationFor", () => {
  const classified = (
    grade: RiskGrade,
    effects: DbSqlClassification["effects"] = [],
  ): DbSqlClassification => ({
    statements: [statement(0, grade, [])],
    grade,
    statementsHash: "h-9",
    effects,
  });

  it("follows the per-grade policy of decision M3-D20", async () => {
    expect(confirmationFor(classified("read"), DEV)).toBeUndefined();
    expect(confirmationFor(classified("write"), DEV)).toBeUndefined();
    for (const grade of ["unboundedWrite", "ddl", "destructive"] as const)
      expect(confirmationFor(classified(grade), DEV)).toMatchObject({
        grade,
        statementsHash: "h-9",
        nameRequired: false,
        rejected: null,
      });
    // 프로덕션은 조회 밖을 모두 묻고, 파괴는 이름을 받는다.
    expect(confirmationFor(classified("read"), PROD)).toBeUndefined();
    expect(confirmationFor(classified("write"), PROD)).toMatchObject({ nameRequired: false });
    expect(confirmationFor(classified("ddl"), PROD)).toMatchObject({ nameRequired: false });
    expect(confirmationFor(classified("destructive"), PROD)).toMatchObject({
      nameRequired: true,
    });
    // 트랜잭션 효과는 등급과 따로 묻는다.
    expect(confirmationFor(classified("write", ["implicitCommit"]), DEV)).toMatchObject({
      effects: ["implicitCommit"],
    });
  });

  it("leaves a read-only connection to the Host — no pre-judgment either way", async () => {
    // Host 는 read-only 연결에서 확인을 묻지 않고 돌리거나 `readOnlyConnection` 으로 거절한다(세션 설정 · 트랜잭션 제어처럼
    // 분류에 없는 것으로도). 화면은 그 거절을 보인다 — 등급만으로 미리 거절하거나 미리 통과시키지 않는다.
    const readOnly = { readOnly: true, production: true };
    for (const grade of ["read", "write", "unboundedWrite", "ddl", "destructive"] as const) {
      expect(confirmationFor(classified(grade), readOnly)).toBeUndefined();
      expect(confirmationFor(classified(grade, ["implicitCommit"]), readOnly)).toBeUndefined();
    }
  });
});

describe("reason wording", () => {
  it("words every RiskReason distinctly in both languages, including replicationReset", async () => {
    for (const strings of [T, EN]) {
      const texts = Object.values(strings.reason);
      expect(texts).toHaveLength(27);
      expect(new Set(texts).size).toBe(texts.length);
      for (const text of texts) expect(text.trim()).not.toBe("");
    }
    expect(T.reason.replicationReset).toBe(
      "바이너리 · 릴레이 로그와 복제 위치를 지운다 — 되돌릴 수 없다",
    );
    expect(T.cancelRollback.title).toBe("취소하면 트랜잭션 전체가 롤백된다");
  });
});

// Redis 명령(M3-18 — `db.kv.*` 의 `3001` 은 `data` 가 `DbKvConfirmRequired`): 문장 목록 대신 명령 한 줄, 글과 바이트를 갈라 보이고,
// 이름 치기 · 처음 포커스 · 돌려주는 값은 SQL 과 같다.
describe("RiskConfirmDialog for a Redis command", () => {
  function kv(over: Partial<DbKvConfirmRequired> = {}): DbKvConfirmRequired {
    return {
      grade: "unboundedWrite",
      action: "delete",
      argv: ["DEL", "0xff", { hex: "0xff" }],
      statementsHash: "kv-1",
      nameRequired: false,
      rejected: null,
      ...over,
    };
  }

  it("shows the command with text and byte arguments apart, no reasons, and [Cancel] first", async () => {
    const answers = await show({ kv: kv(), connection: DEV });
    expect(dialog().dataset.grade).toBe("unboundedWrite");
    expect(dialog().textContent).toContain(T.confirm.title.unboundedWrite);
    expect(dialog().textContent).toContain(T.confirm.kv.command);
    const args = [...document.querySelectorAll<HTMLElement>("[data-testid=db-confirm-kv-arg]")];
    expect(args.map((a) => a.dataset.arg)).toEqual(["text", "text", "bytes"]);
    // 글 "0xff" 는 따옴표 안에, 바이트 0xff 는 표지와 함께 — 같아 보이지 않는다.
    expect(args[1]!.textContent).toBe('"0xff"');
    expect(args[2]!.textContent).toBe(`0xff${T.confirm.kv.bytes}`);
    expect(document.querySelector("[data-testid=db-confirm-statements]")).toBeNull();
    expect(document.querySelector("[data-reason]")).toBeNull();
    expect(document.activeElement).toBe(button("cancel"));
    await act(async () => button("run").click());
    expect(answers).toEqual([{ grade: "unboundedWrite", statementsHash: "kv-1", name: null }]);
  });

  it("asks a production destructive command for the typed connection name", async () => {
    const answers = await show({
      kv: kv({ grade: "destructive", nameRequired: true, statementsHash: "kv-2" }),
      connection: PROD,
    });
    expect(document.activeElement).toBe(button("cancel"));
    expect(button("run").disabled).toBe(true);
    await type(nameInput()!, "prod-billing ");
    expect(button("run").disabled).toBe(true);
    await type(nameInput()!, "prod-billing");
    expect(button("run").disabled).toBe(false);
    await act(async () => button("run").click());
    expect(answers).toEqual([
      { grade: "destructive", statementsHash: "kv-2", name: "prod-billing" },
    ]);
  });

  // 리뷰 M2: 방향 제어 · 0폭 · C1 · BOM 이 든 글 인자는 날것으로 그리지 않는다 — 거꾸로 그려지는 인자, 같아 보이는 다른 키를 막는다.
  it("shows invisible characters in text arguments as code points with a mark", async () => {
    const raw = ["\u202e", "\u2066", "\u200b", "\u2060", "\ufeff", "\u0085", "\u200e", "\u0007"];
    await show({
      kv: kv({ argv: ["DEL", `user:${raw.join("")}gnp.1`, "plain"] }),
      connection: DEV,
    });
    const args = [...document.querySelectorAll<HTMLElement>("[data-testid=db-confirm-kv-arg]")];
    const command = document.querySelector("[data-testid=db-confirm-kv-command]")!.textContent!;
    for (const c of raw) expect(command).not.toContain(c);
    expect(args[1]!.textContent).toBe(
      `"user:⟨U+202E⟩⟨U+2066⟩⟨U+200B⟩⟨U+2060⟩⟨U+FEFF⟩⟨U+0085⟩⟨U+200E⟩⟨U+0007⟩gnp.1"${T.confirm.kv.invisible}`,
    );
    expect(args[1]!.dataset.invisible).toBe("true");
    expect(args[2]!.textContent).toBe('"plain"');
    expect(args[2]!.dataset.invisible).toBeUndefined();
  });

  // 메인 판정(넓힌 showInvisible): ZWJ · ZWNJ · 줄 · 단락 구분자 · 태그 문자도 날것으로 그리지 않는다.
  it("shows joiners, line separators and tag characters in arguments as code points", async () => {
    const raw = ["\u200d", "\u200c", "\u2028", "\u2029", "\u{e0068}", "\u{e0069}"];
    await show({ kv: kv({ argv: ["SET", `k${raw.join("")}`, "v"] }), connection: DEV });
    const args = [...document.querySelectorAll<HTMLElement>("[data-testid=db-confirm-kv-arg]")];
    const command = document.querySelector("[data-testid=db-confirm-kv-command]")!.textContent!;
    for (const c of raw) expect(command).not.toContain(c);
    expect(args[1]!.textContent).toBe(
      `"k⟨U+200D⟩⟨U+200C⟩⟨U+2028⟩⟨U+2029⟩⟨U+E0068⟩⟨U+E0069⟩"${T.confirm.kv.invisible}`,
    );
  });

  it("keeps emoji sequences in arguments as they are, without the mark", async () => {
    await show({ kv: kv({ argv: ["SET", "user:👨‍👩‍👧", "❤️ 🇰🇷 👍🏽", "a\u200db"] }), connection: DEV });
    const args = [...document.querySelectorAll<HTMLElement>("[data-testid=db-confirm-kv-arg]")];
    expect(args.map((a) => a.textContent)).toEqual([
      '"SET"',
      '"user:👨‍👩‍👧"',
      '"❤️ 🇰🇷 👍🏽"',
      `"a⟨U+200D⟩b"${T.confirm.kv.invisible}`,
    ]);
    expect(args.map((a) => a.dataset.invisible)).toEqual([undefined, undefined, undefined, "true"]);
  });

  it("shows the note and cuts a long argument list with a count", async () => {
    const argv = ["DEL", ...Array.from({ length: KV_ARGS_SHOWN + 9 }, (_, i) => `k:${i}`)];
    await show({ kv: kv({ argv }), connection: DEV, note: "the target key is overwritten" });
    expect(document.querySelector("[data-testid=db-confirm-note]")?.textContent).toBe(
      "the target key is overwritten",
    );
    expect(document.querySelectorAll("[data-testid=db-confirm-kv-arg]")).toHaveLength(
      KV_ARGS_SHOWN,
    );
    expect(dialog().textContent).toContain(T.confirm.kv.more(10));
  });
});

describe("RiskConfirmDialog statement text", () => {
  // 리뷰 M2 와 같은 자리: SQL 문장 원문도 그리드와 같은 표지로 보인다(탭 · 줄바꿈은 그대로).
  it("shows invisible characters in a statement as code points, keeping tabs and line breaks", async () => {
    const sql = "DELETE FROM t\tWHERE name = 'a\u202eb\u200b\u{e0041}'\n";
    await show({
      request: request({
        statements: [statement(0, "unboundedWrite", ["missingWhere"], { from: 0, to: sql.length })],
      }),
      connection: DEV,
      sql,
    });
    const text = document.querySelector("[data-testid=db-confirm-statement] pre")!.textContent!;
    expect(text).toBe("DELETE FROM t\tWHERE name = 'a⟨U+202E⟩b⟨U+200B⟩⟨U+E0041⟩'\n");
  });
});
