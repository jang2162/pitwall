// @vitest-environment happy-dom
// MCP 승인 다이얼로그와 「승인 대기」 칸(16-m5-plan M5-14). 가짜 Host 가 `mcp.approvals` 스트림에 요청 · 닫힘을 넣고
// `mcp.approval.answer` 를 모은다. 완료 기준: 요청 → 다이얼로그, 처음 포커스 [거절], 단추마다 보내는 답, `dangerous` ·
// `open_with_upsert` 에 셋째 단추 없음, CLI 요청의 단추 글, 프로젝트 없는 요청의 표시, 대기 둘이 차례로, 시한이 지나면 닫힘, 제어
// 문자 거르기, 파괴 등급은 이름을 치기 전 허용이 꺼짐, `Esc` 뒤 대기 목록에 남음. 그리고 Host 의 닫힘, OS 알림을 청함, 알림을
// 눌러 다시 열기, 목록 상한, 파일 diff. 창의 키맵은 가짜로 둔다.

import { LocalizationProvider } from "@fluent/react";
import type {
  McpApprovalAnswer,
  McpApprovalEvent,
  McpApprovalRequest,
  PlanChange,
  RpcClient,
  SqlStatement,
} from "@pitwall/rpc-client";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, type MockInstance, vi } from "vitest";

import { bundledLocalization, bundledTranslator } from "../i18n/testing";
import { act, endTest, it } from "../test-support/ended-test";
import { agentBlock } from "./agent-text";
import { ARM_DELAY_MS, STATEMENTS_SHOWN } from "./approval-dialog";
import type { ApprovalNative, ApprovalNotice } from "./approval-native";
import { createApprovalStore, type ApprovalStore } from "./approvals";
import { CLOCK_MS, WindowApprovals } from "./approvals-host";
import { LISTED, PendingApprovals } from "./pending-approvals";
import { planDiff } from "./plan-diff";
import { mcpStrings } from "./strings";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const popup = vi.hoisted(() => ({ calls: [] as boolean[] }));
// 문장 원문을 그리는 횟수를 센다(시계가 목록을 다시 그리지 않는지 — G3). 동작은 그대로다.
vi.mock("./agent-text", async (original) => {
  const actual = await original<typeof import("./agent-text")>();
  return { ...actual, agentBlock: vi.fn(actual.agentBlock) };
});

vi.mock("../keymap/keymap-provider", () => ({
  useKeymapPopup: (open: boolean) => popup.calls.push(open),
}));

const T = mcpStrings(bundledTranslator("ko")).approval;
const A = mcpStrings(bundledTranslator("ko")).activity;

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
  vi.useRealTimers();
});

/** `mcp.approvals` 를 흉내 낸 클라이언트: 넣은 이벤트를 스트림으로 내고, 답(`mcp.approval.answer`)을 모은다. */
function fakeHost() {
  const queue: McpApprovalEvent[] = [];
  let wake: (() => void) | undefined;
  const answers: { id: number; answer: McpApprovalAnswer }[] = [];
  const streams: { method: string; params: unknown }[] = [];
  const signals: AbortSignal[] = [];
  const client = {
    stream(method: string, params: unknown, options: { signal: AbortSignal }) {
      streams.push({ method, params });
      signals.push(options.signal);
      options.signal.addEventListener("abort", () => wake?.());
      return (async function* () {
        while (!options.signal.aborted) {
          const next = queue.shift();
          if (next) yield next;
          else await new Promise<void>((resolve) => (wake = resolve));
        }
      })();
    },
    call: vi.fn(async (method: string, params: { id: number; answer: McpApprovalAnswer }) => {
      expect(method).toBe("mcp.approval.answer");
      answers.push({ id: params.id, answer: params.answer });
      return null;
    }),
  };
  return {
    client: client as unknown as RpcClient,
    answers,
    streams,
    /** 끊기지 않은 스트림. 시험은 앱처럼 StrictMode 로 그려 effect 를 걷었다 다시 건다 — 걷힌 쪽의 스트림은 끊긴다. */
    live: () => streams.filter((_, i) => !signals[i]!.aborted),
    async push(...events: McpApprovalEvent[]) {
      queue.push(...events);
      // 마이크로태스크만 흘린다 — 가짜 시계(setTimeout)를 쓰는 시험에서도 돈다.
      await act(async () => {
        wake?.();
        for (let i = 0; i < 50; i++) await Promise.resolve();
      });
    },
  };
}

function fakeNative() {
  const notices: ApprovalNotice[] = [];
  let listener: ((id: number) => void) | undefined;
  const native: ApprovalNative = {
    notify: (notice) => notices.push(notice),
    onReveal: (l) => {
      listener = l;
      return () => (listener = undefined);
    },
  };
  return { native, notices, reveal: (id: number) => act(() => listener?.(id)) };
}

function request(over: Partial<McpApprovalRequest> = {}): McpApprovalRequest {
  return {
    id: 1,
    projectId: "p1",
    notThisProject: false,
    session: "s1",
    client: "claude-code 2.1.0",
    cli: false,
    tool: "fake_write",
    tier: "write",
    plan: {
      method: "test.write",
      tier: "write",
      summary: "write a.txt",
      changes: [],
      rules: { autoApprove: true, remember: { kind: "tool" } },
      hash: "sha256:1",
    },
    answers: ["deny", "once", "session"],
    timeoutSecs: 300,
    expiresAt: Date.now() + 300_000,
    ...over,
  };
}

const open = (r: McpApprovalRequest): McpApprovalEvent => ({ kind: "open", request: r });

/**
 * 잠금 시간은 기본 0(허용 단추가 곧바로 켜진다) — 잠금을 보는 시험만 `ARM_DELAY_MS` 를 준다. `appDefault` 면 prop 을 넘기지 않는다
 * (앱이 쓰는 기본값의 길).
 */
async function mount(
  projectId: string | null = "p1",
  { armDelayMs = 0, appDefault = false }: { armDelayMs?: number; appDefault?: boolean } = {},
) {
  const host = fakeHost();
  const native = fakeNative();
  const store: ApprovalStore = createApprovalStore();
  const element = document.createElement("div");
  document.body.append(element);
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <LocalizationProvider l10n={bundledLocalization("ko")}>
        <WindowApprovals
          client={host.client}
          projectId={projectId}
          native={native.native}
          store={store}
          {...(appDefault ? {} : { armDelayMs })}
        />
        <PendingApprovals projectId="p1" store={store} />
      </LocalizationProvider>,
    ),
  );
  return { ...host, ...native, store };
}

const dialog = () => document.querySelector<HTMLElement>("[data-testid=mcp-approval-dialog]");
const button = (answer: string) =>
  dialog()?.querySelector<HTMLButtonElement>(`[data-answer=${answer}]`) ?? null;
const answerButtons = () =>
  [...(dialog()?.querySelectorAll<HTMLButtonElement>("[data-answer]") ?? [])].map((b) => ({
    answer: b.dataset.answer,
    text: b.textContent,
  }));
const rows = () => [...document.querySelectorAll<HTMLElement>("[data-testid=mcp-approval-row]")];
const text = (testId: string) =>
  dialog()?.querySelector(`[data-testid=${testId}]`)?.textContent ?? null;

function press(key: string, target: Element | null = document.activeElement) {
  act(() => {
    target?.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
  });
}

function type(input: HTMLInputElement, value: string) {
  act(() => {
    const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    set.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

it("a request opens the dialog with the agent, tool and tier; the first focus is Deny", async () => {
  const f = await mount();
  expect(f.live()).toEqual([{ method: "mcp.approvals", params: { projectId: "p1" } }]);
  expect(dialog()).toBeNull();
  await f.push(open(request()));
  expect(dialog()).not.toBeNull();
  expect(dialog()!.dataset.request).toBe("1");
  expect(text("mcp-approval-client")).toBe("claude-code 2.1.0");
  expect(text("mcp-approval-tool")).toBe("fake_write");
  expect(text("mcp-approval-tier")).toBe(T.tier.write);
  expect(text("mcp-approval-summary")).toBe("write a.txt");
  expect(text("mcp-approval-position")).toBe(T.position(1, 1));
  expect(document.activeElement).toBe(button("deny"));
  expect(popup.calls).toContain(true);
  expect(answerButtons().map((b) => b.answer)).toEqual(["deny", "once", "session"]);
});

it.each<{ answer: McpApprovalAnswer }>([
  { answer: "deny" },
  { answer: "once" },
  { answer: "session" },
])("each button sends its answer and closes the dialog", async ({ answer }) => {
  const f = await mount();
  await f.push(open(request({ id: 7 })));
  await act(async () => button(answer)!.click());
  expect(f.answers).toEqual([{ id: 7, answer }]);
  expect(dialog()).toBeNull();
  expect(rows()).toHaveLength(0);
});

it("a CLI request's third button allows the CLI while the window is open", async () => {
  const f = await mount();
  await f.push(
    open(request({ cli: true, client: "pitwall-cli 0.1.0", answers: ["deny", "once", "window"] })),
  );
  expect(answerButtons()).toEqual([
    { answer: "deny", text: T.deny },
    { answer: "once", text: T.once },
    { answer: "window", text: T.window },
  ]);
  expect(text("mcp-approval-cli")).toBe(T.cli);
  await act(async () => button("window")!.click());
  expect(f.answers).toEqual([{ id: 1, answer: "window" }]);
});

it("dangerous tools and plans that are never remembered have no third button", async () => {
  const f = await mount();
  // Host 가 셋째 답을 실어 보내도(잘못) 그리지 않는다 — 등급 · 계획 규칙도 본다.
  await f.push(
    open(request({ id: 1, tier: "dangerous", tool: "db_execute" })),
    open(
      request({
        id: 2,
        tool: "open_with_upsert",
        plan: {
          ...request().plan,
          rules: { autoApprove: false, remember: { kind: "never" } },
        },
      }),
    ),
  );
  expect(answerButtons().map((b) => b.answer)).toEqual(["deny", "once"]);
  expect(dialog()!.dataset.tier).toBe("dangerous");
  await act(async () => button("deny")!.click());
  expect(dialog()!.dataset.request).toBe("2");
  expect(answerButtons().map((b) => b.answer)).toEqual(["deny", "once"]);
  // Host 가 내지 않은 답도 그리지 않는다.
  await act(async () => button("deny")!.click());
  await f.push(open(request({ id: 3, answers: ["deny", "once"] })));
  expect(answerButtons().map((b) => b.answer)).toEqual(["deny", "once"]);
});

it("a request without a project says it is not this project's", async () => {
  const f = await mount();
  await f.push(open(request({ projectId: null, notThisProject: true, tool: "project_register" })));
  expect(text("mcp-approval-not-this-project")).toBe(T.notThisProject);
  await f.push(open(request({ id: 2 })));
  await act(async () => button("deny")!.click());
  expect(dialog()!.querySelector("[data-testid=mcp-approval-not-this-project]")).toBeNull();
});

it("a window without a project listens without one", async () => {
  const f = await mount(null);
  expect(f.live()).toEqual([{ method: "mcp.approvals", params: {} }]);
});

it("two pending requests are shown one after the other", async () => {
  const f = await mount();
  await f.push(open(request({ id: 2, tool: "second" })), open(request({ id: 1, tool: "first" })));
  expect(text("mcp-approval-tool")).toBe("first");
  expect(text("mcp-approval-position")).toBe(T.position(1, 2));
  expect(rows().map((r) => r.dataset.request)).toEqual(["1", "2"]);
  await act(async () => button("once")!.click());
  expect(text("mcp-approval-tool")).toBe("second");
  expect(text("mcp-approval-position")).toBe(T.position(1, 1));
  expect(document.activeElement).toBe(button("deny"));
  await act(async () => button("deny")!.click());
  expect(f.answers).toEqual([
    { id: 1, answer: "once" },
    { id: 2, answer: "deny" },
  ]);
  expect(dialog()).toBeNull();
});

it("the Host withdrawing a request closes its dialog", async () => {
  const f = await mount();
  await f.push(open(request()));
  await f.push({ kind: "closed", id: 1 });
  expect(dialog()).toBeNull();
  expect(rows()).toHaveLength(0);
  expect(document.querySelector("[data-testid=mcp-approvals-empty]")?.textContent).toBe(
    A.approvalsEmpty,
  );
});

it("the dialog shows the time left and closes when it runs out", async () => {
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
  vi.setSystemTime(1_000_000);
  const f = await mount();
  await f.push(open(request({ expiresAt: 1_000_000 + 65_000 })));
  expect(text("mcp-approval-remaining")).toBe(T.remaining("1:05"));
  await act(async () => vi.advanceTimersByTime(CLOCK_MS * 6));
  expect(text("mcp-approval-remaining")).toBe(T.remaining("0:59"));
  expect(rows()[0]!.textContent).toContain("0:59");
  await act(async () => vi.advanceTimersByTime(60_000));
  expect(dialog()).toBeNull();
  expect(rows()).toHaveLength(0);
  expect(f.answers).toEqual([]);
});

it("text the agent sent loses its control, direction and zero-width characters", async () => {
  const f = await mount();
  const plan = request().plan;
  await f.push(
    open(
      request({
        client: "claude\u200bcode\u202e edoc\n[Allow]",
        tool: "fake_write",
        plan: {
          ...plan,
          summary: "write\u0007 a.txt\r\nProject: other",
          changes: [{ kind: "text", label: "cmd\u2066", text: "rm -rf /\u200d\nnext line" }],
        },
      }),
    ),
  );
  expect(text("mcp-approval-client")).toBe("claude⟨U+200B⟩code⟨U+202E⟩ edoc⟨U+000A⟩[Allow]");
  expect(dialog()!.querySelector("[data-testid=mcp-approval-client-invisible]")).not.toBeNull();
  expect(text("mcp-approval-summary")).toBe("write⟨U+0007⟩ a.txt⟨U+000D⟩⟨U+000A⟩Project: other");
  expect(text("mcp-approval-text-label")).toBe("cmd⟨U+2066⟩");
  // 여러 줄 자리는 줄바꿈을 두고 보이지 않는 글자만 표지로.
  expect(text("mcp-approval-text-body")).toBe("rm -rf /⟨U+200D⟩\nnext line");
  // 단추처럼 꾸민 글은 단추가 되지 않는다.
  expect(answerButtons().map((b) => b.answer)).toEqual(["deny", "once", "session"]);
  // 목록도 같은 규칙.
  expect(rows()[0]!.textContent).toContain("claude⟨U+200B⟩code⟨U+202E⟩");
  // 알림 글도.
  expect(f.notices[0]!.body).toBe(
    T.notifyBody("claude⟨U+200B⟩code⟨U+202E⟩ edoc⟨U+000A⟩[Allow]", "fake_write"),
  );
});

it("a long agent text is cut and does not push the buttons away", async () => {
  const f = await mount();
  await f.push(
    open(
      request({ client: "a".repeat(1000), plan: { ...request().plan, summary: "s".repeat(5000) } }),
    ),
  );
  expect(text("mcp-approval-client")).toBe(`${"a".repeat(200)}…`);
  expect(text("mcp-approval-summary")).toBe(`${"s".repeat(500)}…`);
  expect(button("deny")).not.toBeNull();
});

function sql(statements: SqlStatement[], over: Partial<Extract<PlanChange, { kind: "sql" }>> = {}) {
  return {
    kind: "sql",
    connection: { name: "prod-main", production: true, readOnly: false },
    file: null,
    statements,
    irreversible: false,
    ...over,
  } satisfies PlanChange;
}

const stmt = (index: number, grade: SqlStatement["grade"], text: string): SqlStatement => ({
  index,
  line: index + 1,
  grade,
  text,
});

it("a destructive SQL statement needs the connection name before allowing", async () => {
  const f = await mount();
  await f.push(
    open(
      request({
        tier: "dangerous",
        tool: "db_execute",
        answers: ["deny", "once"],
        plan: {
          ...request().plan,
          tier: "dangerous",
          changes: [
            sql(
              [
                stmt(0, "write", "UPDATE t SET a = 1 WHERE id = 1"),
                stmt(1, "destructive", "DROP TABLE t"),
              ],
              {
                file: { path: "/w/migrate.sql", sha256: "ab" },
                irreversible: true,
              },
            ),
          ],
        },
      }),
    ),
  );
  const statements = [
    ...dialog()!.querySelectorAll<HTMLElement>("[data-testid=mcp-approval-statement]"),
  ];
  expect(statements.map((s) => s.dataset.grade)).toEqual(["write", "destructive"]);
  expect(dialog()!.querySelector("[data-testid=db-connection-mark]")?.textContent).toContain(
    "prod-main",
  );
  expect(text("mcp-approval-sql-file")).toBe("/w/migrate.sql");
  expect(text("mcp-approval-irreversible")).toBe(T.sql.irreversible);
  // 처음 포커스는 이름 칸이 아니라 [거절].
  expect(document.activeElement).toBe(button("deny"));
  expect(button("once")!.disabled).toBe(true);
  const name = dialog()!.querySelector<HTMLInputElement>("[data-testid=mcp-approval-name]")!;
  type(name, "prod-mai");
  expect(button("once")!.disabled).toBe(true);
  await act(async () => button("once")!.click());
  expect(f.answers).toEqual([]);
  type(name, "prod-main");
  expect(button("once")!.disabled).toBe(false);
  await act(async () => button("once")!.click());
  expect(f.answers).toEqual([{ id: 1, answer: "once" }]);
});

it("SQL without a destructive statement does not ask for the name", async () => {
  const f = await mount();
  await f.push(
    open(
      request({
        tier: "dangerous",
        answers: ["deny", "once"],
        plan: {
          ...request().plan,
          changes: [
            sql([stmt(0, "write", "INSERT INTO t VALUES (1)")], {
              connection: { name: "dev", production: false, readOnly: false },
            }),
          ],
        },
      }),
    ),
  );
  expect(dialog()!.querySelector("[data-testid=mcp-approval-name]")).toBeNull();
  expect(button("once")!.disabled).toBe(false);
});

const grades = () =>
  [...dialog()!.querySelectorAll<HTMLElement>("[data-testid=mcp-approval-statement]")].map(
    (s) => s.dataset.grade,
  );

it("statements that are not reads are all shown; only reads past the limit are folded", async () => {
  const f = await mount();
  // 쓰기 59개 사이 56번째(0 기준 55)가 파괴: 50개 밖이어도 보인다(M5-14 리뷰 F1).
  const writes = Array.from({ length: 60 }, (_, i) =>
    i === 55
      ? stmt(i, "destructive", "DROP TABLE users")
      : stmt(i, "write", `UPDATE t SET a = ${i} WHERE id = ${i}`),
  );
  await f.push(
    open(
      request({
        tier: "dangerous",
        answers: ["deny", "once"],
        plan: { ...request().plan, changes: [sql(writes)] },
      }),
    ),
  );
  expect(grades()).toHaveLength(60);
  expect(grades()[55]).toBe("destructive");
  expect(dialog()!.textContent).toContain("DROP TABLE users");
  expect(dialog()!.querySelector("[data-testid=mcp-approval-folded]")).toBeNull();
  await act(async () => button("deny")!.click());
  // 조회 문장은 앞 `STATEMENTS_SHOWN` 개만, 넘는 것은 접고 수를 보인다. 조회 사이의 쓰기 · 파괴는 그대로 보인다.
  const mixed = Array.from({ length: STATEMENTS_SHOWN + 10 }, (_, i) =>
    stmt(i, "read", `SELECT ${i}`),
  );
  mixed.push(stmt(STATEMENTS_SHOWN + 10, "destructive", "TRUNCATE audit"));
  mixed.push(stmt(STATEMENTS_SHOWN + 11, "read", "SELECT last"));
  await f.push(
    open(
      request({
        id: 2,
        tier: "dangerous",
        answers: ["deny", "once"],
        plan: { ...request().plan, changes: [sql(mixed)] },
      }),
    ),
  );
  expect(grades().filter((g) => g === "read")).toHaveLength(STATEMENTS_SHOWN);
  expect(grades().filter((g) => g === "destructive")).toHaveLength(1);
  expect(dialog()!.textContent).toContain("TRUNCATE audit");
  expect(text("mcp-approval-folded")).toBe(T.sql.foldedReads(11));
});

it("Escape only closes the dialog; the request stays in the pending list and reopens from it", async () => {
  const f = await mount();
  await f.push(open(request({ id: 1 })), open(request({ id: 2, tool: "second" })));
  press("Escape", dialog());
  // 다음 대기가 뜬다 — 닫은 요청은 답하지 않았다.
  expect(f.answers).toEqual([]);
  expect(dialog()!.dataset.request).toBe("2");
  press("Escape", dialog());
  expect(dialog()).toBeNull();
  expect(rows().map((r) => r.dataset.request)).toEqual(["1", "2"]);
  await act(async () => rows()[1]!.click());
  expect(dialog()!.dataset.request).toBe("2");
  expect(document.activeElement).toBe(button("deny"));
  await act(async () => button("deny")!.click());
  expect(dialog()).toBeNull();
  expect(rows().map((r) => r.dataset.request)).toEqual(["1"]);
  await act(async () => rows()[0]!.click());
  expect(dialog()!.dataset.request).toBe("1");
});

it("clicking outside the dialog does not close it", async () => {
  const f = await mount();
  await f.push(open(request()));
  const outside = document.createElement("button");
  document.body.append(outside);
  act(() => {
    outside.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, cancelable: true }));
    outside.dispatchEvent(new FocusEvent("focusin", { bubbles: true }));
  });
  await act(async () => outside.click());
  expect(dialog()).not.toBeNull();
  expect(f.answers).toEqual([]);
  expect(rows().map((r) => r.dataset.request)).toEqual(["1"]);
});

it("every new request asks for an OS notification; clicking it reopens that dialog", async () => {
  const f = await mount();
  await f.push(open(request({ id: 3, client: null, tool: "project_register" })));
  expect(f.notices).toEqual([
    { request: 3, title: T.notifyTitle, body: T.notifyBody(T.notifyAgent, "project_register") },
  ]);
  press("Escape", dialog());
  expect(dialog()).toBeNull();
  f.reveal(3);
  expect(dialog()!.dataset.request).toBe("3");
  // 이미 끝난 요청의 알림은 아무것도 열지 않는다.
  await act(async () => button("deny")!.click());
  f.reveal(3);
  expect(dialog()).toBeNull();
  // 듣기를 다시 열어 같은 요청이 다시 와도 알림을 다시 청하지 않는다.
  await f.push(open(request({ id: 4 })), open(request({ id: 4 })));
  expect(f.notices.map((n) => n.request)).toEqual([3, 4]);
});

it("the pending list draws its first rows and counts the rest", async () => {
  const f = await mount();
  await f.push(
    ...Array.from({ length: LISTED + 4 }, (_, i) => open(request({ id: i + 1, session: `s${i}` }))),
  );
  expect(rows()).toHaveLength(LISTED);
  expect(document.querySelector("[data-testid=mcp-approvals-more]")?.textContent).toBe(
    A.approvalsMore(4),
  );
  expect(text("mcp-approval-position")).toBe(T.position(1, LISTED + 4));
});

it("a file change is drawn with the diff view", async () => {
  const f = await mount();
  await f.push(
    open(
      request({
        plan: {
          ...request().plan,
          changes: [
            { kind: "file", path: "/w/a.txt", before: "one\ntwo\n", after: "one\nt\u200bwo\n" },
          ],
        },
      }),
    ),
  );
  const file = dialog()!.querySelector<HTMLElement>("[data-testid=mcp-approval-file]")!;
  expect(file.dataset.status).toBe("modified");
  expect(text("mcp-approval-path")).toBe("/w/a.txt");
  expect(file.querySelector(".cm-editor")).not.toBeNull();
  expect(file.textContent).toContain("t⟨U+200B⟩wo");
});

// 16-m5-plan M5-16 의 두 툴이 내는 계획 꼴 그대로(`pw_core::registry::tool` 의 `PlannedRegistration::plan`,
// `pw_open_with::upsert` 의 `PlannedUpsert::plan`): 인용된 값이 든 넉 줄 글과, 옛 명령을 덮는 `settings.toml` 의 앞뒤.
it("the project_register and open_with_upsert plans are shown as the tools write them", async () => {
  const f = await mount();
  const registerText = [
    'root: "/w/say \\"hi\\""',
    'name: "beta\\ntrusted: yes"',
    'settings: repository ("/w/say \\"hi\\"/.pitwall", committed with the code)',
    "trusted: no",
  ].join("\n");
  const before =
    'schema = 1\n\n[open_with.tools.zed]\ncommand = ["/opt/old-zed", "{project_root}"]\n';
  const after = 'schema = 1\n\n[open_with.tools.zed]\ncommand = ["/opt/zed", "{project_root}"]\n';
  await f.push(
    open(
      request({
        id: 1,
        projectId: null,
        notThisProject: true,
        tool: "project_register",
        plan: {
          ...request().plan,
          method: "project.tool.register",
          summary: 'Register "/w/say \\"hi\\"" as the project "beta\\ntrusted: yes"',
          changes: [{ kind: "text", label: "project", text: registerText }],
        },
      }),
    ),
    open(
      request({
        id: 2,
        projectId: null,
        notThisProject: true,
        tool: "open_with_upsert",
        answers: ["deny", "once"],
        plan: {
          ...request().plan,
          method: "openWith.tool.upsert",
          summary: "Change the external tool `zed` in the global settings",
          changes: [{ kind: "file", path: "/c/settings.toml", before, after }],
          rules: { autoApprove: false, remember: { kind: "never" } },
        },
      }),
    ),
  );
  // 인용이 그대로 보이고(이스케이프한 `\n` 은 글자 둘이다) 줄은 넷이다 — 이름이 가짜 `trusted: yes` 줄을 만들지 못한다.
  expect(text("mcp-approval-text-label")).toBe("project");
  const body = text("mcp-approval-text-body") ?? "";
  expect(body).toContain('name: "beta\\ntrusted: yes"');
  expect(body.split("\n").filter((line) => line.startsWith("trusted:"))).toEqual(["trusted: no"]);
  expect(text("mcp-approval-not-this-project")).toBe(T.notThisProject);
  await act(async () => button("deny")!.click());

  expect(text("mcp-approval-tool")).toBe("open_with_upsert");
  const file = dialog()!.querySelector<HTMLElement>("[data-testid=mcp-approval-file]")!;
  expect(file.dataset.status).toBe("modified");
  expect(text("mcp-approval-path")).toBe("/c/settings.toml");
  // 덮이는 옛 명령과 새 명령이 함께 보인다.
  expect(file.textContent).toContain("/opt/old-zed");
  expect(file.textContent).toContain("/opt/zed");
  expect(answerButtons().map((b) => b.answer)).toEqual(["deny", "once"]);
});

it("planDiff turns the before and after text into hunks", async () => {
  const changed = planDiff("/w/a.txt", "a\nb\nc\n", "a\nB\nc\nd");
  expect(changed.file.status).toBe("modified");
  expect(changed.file.hunks.map((h) => [h.oldStart, h.oldLines, h.newStart, h.newLines])).toEqual([
    [2, 1, 2, 1],
    [3, 0, 4, 1],
  ]);
  expect(changed.file.hunks[1]!.lines).toEqual([
    { kind: "added", text: "d", oldLine: null, newLine: 4, noNewline: true },
  ]);
  expect([changed.file.added, changed.file.removed]).toEqual([2, 1]);
  const added = planDiff("/w/new.txt", null, "x\n");
  expect(added.file.status).toBe("added");
  expect(added.file.hunks.map((h) => [h.oldStart, h.oldLines, h.newStart, h.newLines])).toEqual([
    [0, 0, 1, 1],
  ]);
  expect(planDiff("/w/gone.txt", "x\n", null).file.status).toBe("deleted");
  // 보이지 않는 글자는 두 쪽 모두 표지로.
  expect(planDiff("/w/a", "a\u200b\n", "a\n").oldText).toBe("a⟨U+200B⟩\n");
});

// --- 허용 단추의 잠금(M5-14 리뷰 B1). 가짜 시계로 `ARM_DELAY_MS` 를 센다. ---

const fakeClock = () =>
  vi.useFakeTimers({
    toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"],
  });
const advance = (ms: number) => act(async () => vi.advanceTimersByTime(ms));
const clickAt = (el: HTMLElement) =>
  act(async () => {
    // 포인터로 누른 클릭(`detail` 1) — 누름은 따로 보낸다(`pointerDown`).
    el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 }));
  });
const pointerDown = (el: Element) =>
  act(() => {
    el.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, cancelable: true }));
  });

it("allow buttons are locked for a moment after a request shows; Deny works at once", async () => {
  fakeClock();
  const f = await mount("p1", { armDelayMs: ARM_DELAY_MS });
  await f.push(open(request({ id: 1 })), open(request({ id: 2 })));
  expect(button("once")!.disabled).toBe(true);
  expect(button("session")!.disabled).toBe(true);
  expect(button("once")!.dataset.locked).toBe("true");
  expect(button("once")!.querySelector("[data-testid=mcp-approval-lock]")).not.toBeNull();
  expect(button("once")!.title).toBe(T.locked);
  expect(button("deny")!.disabled).toBe(false);
  // 잠긴 동안의 Enter(키보드 누름 — `detail` 0)는 무시된다.
  await act(async () => button("once")!.click());
  expect(f.answers).toEqual([]);
  await advance(ARM_DELAY_MS - 1);
  expect(button("once")!.disabled).toBe(true);
  await advance(1);
  expect(button("once")!.disabled).toBe(false);
  expect(button("once")!.dataset.locked).toBeUndefined();
  expect(button("once")!.querySelector("[data-testid=mcp-approval-lock]")).toBeNull();
  // [거절] 은 뜬 순간에도 된다 — 둘째 요청에서 본다.
  await act(async () => button("once")!.click());
  expect(f.answers).toEqual([{ id: 1, answer: "once" }]);
  expect(button("once")!.disabled).toBe(true);
  await act(async () => button("deny")!.click());
  expect(f.answers).toEqual([
    { id: 1, answer: "once" },
    { id: 2, answer: "deny" },
  ]);
});

it("typing Tab then Enter while the dialog appears does not allow", async () => {
  fakeClock();
  const f = await mount("p1", { armDelayMs: ARM_DELAY_MS });
  await f.push(open(request()));
  // 에디터에서 들여쓰기를 치던 사람: 키를 누를 때마다 잠금을 처음부터 다시 센다.
  for (let i = 0; i < 4; i++) {
    await advance(ARM_DELAY_MS / 2);
    press("Tab", dialog());
    press("Enter", button("once"));
    await act(async () => button("once")!.click());
  }
  expect(f.answers).toEqual([]);
  expect(button("once")!.disabled).toBe(true);
  // 키가 멈추고 잠금 시간이 지나야 켜진다.
  await advance(ARM_DELAY_MS - 1);
  expect(button("once")!.disabled).toBe(true);
  await advance(1);
  expect(button("once")!.disabled).toBe(false);
});

it("a double click does not answer the next request too", async () => {
  fakeClock();
  const f = await mount("p1", { armDelayMs: ARM_DELAY_MS });
  await f.push(open(request({ id: 1 })), open(request({ id: 2, tool: "second" })));
  await advance(ARM_DELAY_MS);
  pointerDown(button("once")!);
  await clickAt(button("once")!);
  expect(f.answers).toEqual([{ id: 1, answer: "once" }]);
  // 같은 자리의 둘째 누름 — 새 요청의 [이번만 허용] 은 다시 잠겼다.
  expect(dialog()!.dataset.request).toBe("2");
  pointerDown(button("once")!);
  await clickAt(button("once")!);
  expect(f.answers).toEqual([{ id: 1, answer: "once" }]);
  expect(button("once")!.disabled).toBe(true);
});

it("a click meant for a request the Host just closed does not answer the next one", async () => {
  fakeClock();
  const f = await mount("p1", { armDelayMs: ARM_DELAY_MS });
  await f.push(open(request({ id: 1 })), open(request({ id: 2, tool: "second" })));
  await advance(ARM_DELAY_MS);
  // 사람이 #1 의 [이번만 허용] 에 손을 내리는 순간 Host 가 #1 을 거둔다(시한 · 세션 끊김).
  const target = button("once")!;
  await f.push({ kind: "closed", id: 1 });
  expect(dialog()!.dataset.request).toBe("2");
  await clickAt(target.isConnected ? target : button("once")!);
  await act(async () => button("once")!.click());
  expect(f.answers).toEqual([]);
});

it("a mouse press that began before the lock ended does not allow on release", async () => {
  fakeClock();
  const f = await mount("p1", { armDelayMs: ARM_DELAY_MS });
  await f.push(open(request()));
  await advance(ARM_DELAY_MS / 2);
  // 잠긴 동안 내려간 누름은 잠금을 다시 센다 — 떼기는 풀린 뒤에 온다.
  pointerDown(button("once")!);
  await advance(ARM_DELAY_MS);
  expect(button("once")!.disabled).toBe(false);
  await clickAt(button("once")!);
  expect(f.answers).toEqual([]);
  // 풀린 뒤에 그 단추에서 다시 누르면 받는다.
  pointerDown(button("once")!);
  await clickAt(button("once")!);
  expect(f.answers).toEqual([{ id: 1, answer: "once" }]);
});

it("reopening a dismissed request locks its allow buttons again", async () => {
  fakeClock();
  const f = await mount("p1", { armDelayMs: ARM_DELAY_MS });
  await f.push(open(request({ id: 5 })));
  await advance(ARM_DELAY_MS);
  expect(button("once")!.disabled).toBe(false);
  press("Escape", dialog());
  f.reveal(5);
  expect(dialog()!.dataset.request).toBe("5");
  expect(button("once")!.disabled).toBe(true);
  await act(async () => rows()[0]!.click());
  await advance(ARM_DELAY_MS);
  expect(button("once")!.disabled).toBe(false);
});

it("typing the connection name does not keep the buttons locked", async () => {
  fakeClock();
  const f = await mount("p1", { armDelayMs: ARM_DELAY_MS });
  await f.push(
    open(
      request({
        tier: "dangerous",
        answers: ["deny", "once"],
        plan: { ...request().plan, changes: [sql([stmt(0, "destructive", "DROP TABLE t")])] },
      }),
    ),
  );
  const name = dialog()!.querySelector<HTMLInputElement>("[data-testid=mcp-approval-name]")!;
  await advance(ARM_DELAY_MS / 2);
  press("p", name);
  type(name, "prod-main");
  await advance(ARM_DELAY_MS / 2);
  expect(button("once")!.disabled).toBe(false);
  await act(async () => button("once")!.click());
  expect(f.answers).toEqual([{ id: 1, answer: "once" }]);
});

// --- 이름 칸(M5-14 리뷰 F3). ---

const destructive = (id: number, name = "prod-main") =>
  request({
    id,
    tier: "dangerous",
    answers: ["deny", "once"],
    plan: {
      ...request().plan,
      changes: [
        sql([stmt(0, "destructive", "DROP TABLE t")], {
          connection: { name, production: true, readOnly: false },
        }),
      ],
    },
  });

it("a typed name does not carry over to the next request on the same connection", async () => {
  const f = await mount();
  await f.push(open(destructive(1)), open(destructive(2)));
  type(dialog()!.querySelector<HTMLInputElement>("[data-testid=mcp-approval-name]")!, "prod-main");
  await act(async () => button("once")!.click());
  expect(f.answers).toEqual([{ id: 1, answer: "once" }]);
  expect(dialog()!.dataset.request).toBe("2");
  expect(dialog()!.querySelector<HTMLInputElement>("[data-testid=mcp-approval-name]")!.value).toBe(
    "",
  );
  expect(button("once")!.disabled).toBe(true);
  await act(async () => button("once")!.click());
  expect(f.answers).toEqual([{ id: 1, answer: "once" }]);
});

it("the name must match exactly — no trimming, case folding or look-alike characters", async () => {
  const f = await mount();
  await f.push(open(destructive(1)));
  const name = dialog()!.querySelector<HTMLInputElement>("[data-testid=mcp-approval-name]")!;
  for (const v of [
    " prod-main",
    "prod-main ",
    "PROD-MAIN",
    "Prod-main",
    "prod‐main",
    "prod-main​",
    "prod–main",
    "prod-mai",
  ]) {
    type(name, v);
    expect(button("once")!.disabled, JSON.stringify(v)).toBe(true);
  }
  type(name, "prod-main");
  expect(button("once")!.disabled).toBe(false);
});

it("every connection with a destructive statement needs its own name", async () => {
  const f = await mount();
  await f.push(
    open(
      request({
        tier: "dangerous",
        answers: ["deny", "once"],
        plan: {
          ...request().plan,
          changes: [
            sql([stmt(0, "destructive", "DROP TABLE a")], {
              connection: { name: "prod-a", production: true, readOnly: false },
            }),
            sql([stmt(0, "destructive", "DROP TABLE b")], {
              connection: { name: "prod-b", production: true, readOnly: false },
            }),
          ],
        },
      }),
    ),
  );
  const names = [
    ...dialog()!.querySelectorAll<HTMLInputElement>("[data-testid=mcp-approval-name]"),
  ];
  expect(names).toHaveLength(2);
  type(names[0]!, "prod-a");
  expect(button("once")!.disabled).toBe(true);
  type(names[1]!, "prod-b");
  expect(button("once")!.disabled).toBe(false);
});

it("a reopened later request shows its own place in the queue", async () => {
  const f = await mount();
  await f.push(open(request({ id: 1 })), open(request({ id: 2, tool: "second" })));
  press("Escape", dialog());
  press("Escape", dialog());
  await act(async () => rows()[1]!.click());
  expect(text("mcp-approval-position")).toBe(T.position(2, 2));
});

// --- 잠금은 사람이 그 요청을 볼 수 있게 된 순간부터 센다(M5-14 재리뷰 G1 · G2). ---

/** 창이 뒤로 가거나(`blur`) 앞으로 온다(`focus`). happy-dom 의 `document.hasFocus()` 는 늘 참이라 함께 바꾼다. */
function windowEvent(type: "blur" | "focus") {
  focusSpy ??= vi.spyOn(document, "hasFocus");
  focusSpy.mockReturnValue(type === "focus");
  act(() => {
    window.dispatchEvent(new FocusEvent(type));
  });
}

let focusSpy: MockInstance<() => boolean> | undefined;
afterEach(() => {
  focusSpy?.mockRestore();
  focusSpy = undefined;
});

function setVisibility(state: "hidden" | "visible") {
  Object.defineProperty(document, "visibilityState", { configurable: true, get: () => state });
  act(() => {
    document.dispatchEvent(new Event("visibilitychange"));
  });
}

it("revealing the request already on screen locks its allow buttons again", async () => {
  fakeClock();
  const f = await mount("p1", { armDelayMs: ARM_DELAY_MS });
  await f.push(open(request({ id: 5 })));
  await advance(ARM_DELAY_MS);
  expect(button("once")!.disabled).toBe(false);
  // 알림을 눌러 창이 앞에 온다 — 같은 요청이 이미 떠 있다(탐침 C).
  f.reveal(5);
  expect(button("once")!.disabled).toBe(true);
  await advance(ARM_DELAY_MS);
  expect(button("once")!.disabled).toBe(false);
  // 「승인 대기」 줄을 눌러도 같다.
  await act(async () => rows()[0]!.click());
  expect(button("once")!.disabled).toBe(true);
});

it("the lock does not run out while the window is in the back; coming to the front locks again", async () => {
  fakeClock();
  const f = await mount("p1", { armDelayMs: ARM_DELAY_MS });
  // 창이 뒤에 있을 때 온 요청: 시간이 지나도 풀리지 않는다.
  windowEvent("blur");
  await f.push(open(request({ id: 5 })));
  await advance(ARM_DELAY_MS * 3);
  expect(button("once")!.disabled).toBe(true);
  // 앞으로 오면 처음부터 센다(탐침 D).
  windowEvent("focus");
  await advance(ARM_DELAY_MS - 1);
  expect(button("once")!.disabled).toBe(true);
  await advance(1);
  expect(button("once")!.disabled).toBe(false);
  // 풀린 뒤 창이 뒤로 갔다 오면 다시 잠긴다.
  windowEvent("blur");
  expect(button("once")!.disabled).toBe(true);
  await advance(ARM_DELAY_MS * 3);
  windowEvent("focus");
  expect(button("once")!.disabled).toBe(true);
  await advance(ARM_DELAY_MS);
  expect(button("once")!.disabled).toBe(false);
  expect(f.answers).toEqual([]);
});

it("a hidden window keeps the lock and showing it again starts the count over", async () => {
  fakeClock();
  const f = await mount("p1", { armDelayMs: ARM_DELAY_MS });
  try {
    await f.push(open(request()));
    await advance(ARM_DELAY_MS);
    setVisibility("hidden");
    expect(button("once")!.disabled).toBe(true);
    await advance(ARM_DELAY_MS * 3);
    expect(button("once")!.disabled).toBe(true);
    setVisibility("visible");
    expect(button("once")!.disabled).toBe(true);
    await advance(ARM_DELAY_MS);
    expect(button("once")!.disabled).toBe(false);
  } finally {
    Reflect.deleteProperty(document, "visibilityState");
  }
});

it("the app's default lock is ARM_DELAY_MS", async () => {
  expect(ARM_DELAY_MS).toBeGreaterThanOrEqual(500);
  fakeClock();
  const f = await mount("p1", { appDefault: true });
  await f.push(open(request()));
  expect(button("once")!.disabled).toBe(true);
  await advance(ARM_DELAY_MS - 1);
  expect(button("once")!.disabled).toBe(true);
  await advance(1);
  expect(button("once")!.disabled).toBe(false);
});

// --- 시계는 목록을 다시 그리지 않는다(M5-14 재리뷰 G3). ---

it("with 5,000 statements the clock tick does not redraw the statement list", async () => {
  fakeClock();
  const f = await mount("p1", { armDelayMs: ARM_DELAY_MS });
  const many = Array.from({ length: 5000 }, (_, i) =>
    stmt(i, "write", `UPDATE t SET a = ${i} WHERE id = ${i}`),
  );
  const drawn = vi.mocked(agentBlock);
  drawn.mockClear();
  await f.push(
    open(
      request({
        tier: "dangerous",
        answers: ["deny", "once"],
        expiresAt: Date.now() + 300_000,
        plan: { ...request().plan, changes: [sql(many)] },
      }),
    ),
  );
  const first = drawn.mock.calls.length;
  expect(first).toBeGreaterThanOrEqual(5000);
  const before = text("mcp-approval-remaining");
  // 시계 셋 · 잠금 풀림 하나 — 다이얼로그는 다시 그려지지만 문장 목록은 그대로다.
  await advance(CLOCK_MS * 3);
  expect(text("mcp-approval-remaining")).not.toBe(before);
  expect(button("once")!.disabled).toBe(false);
  expect(drawn.mock.calls.length).toBe(first);
});
