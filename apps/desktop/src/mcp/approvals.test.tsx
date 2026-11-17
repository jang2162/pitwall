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
import { afterEach, beforeEach, expect, vi } from "vitest";

import { bundledLocalization, bundledTranslator } from "../i18n/testing";
import { act, endTest, it } from "../test-support/ended-test";
import { STATEMENTS_SHOWN } from "./approval-dialog";
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
  const client = {
    stream(method: string, params: unknown, options: { signal: AbortSignal }) {
      streams.push({ method, params });
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
    async push(...events: McpApprovalEvent[]) {
      queue.push(...events);
      await act(async () => {
        wake?.();
        await new Promise((resolve) => setTimeout(resolve, 0));
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

async function mount(projectId: string | null = "p1") {
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
  expect(f.streams).toEqual([{ method: "mcp.approvals", params: { projectId: "p1" } }]);
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
  expect(f.streams).toEqual([{ method: "mcp.approvals", params: {} }]);
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

it("SQL without a destructive statement does not ask for the name; long lists are cut", async () => {
  const f = await mount();
  const many = Array.from({ length: STATEMENTS_SHOWN + 3 }, (_, i) =>
    stmt(i, "write", `INSERT INTO t VALUES (${i})`),
  );
  await f.push(
    open(
      request({
        tier: "dangerous",
        answers: ["deny", "once"],
        plan: {
          ...request().plan,
          changes: [sql(many, { connection: { name: "dev", production: false, readOnly: false } })],
        },
      }),
    ),
  );
  expect(dialog()!.querySelector("[data-testid=mcp-approval-name]")).toBeNull();
  expect(button("once")!.disabled).toBe(false);
  expect(dialog()!.querySelectorAll("[data-testid=mcp-approval-statement]")).toHaveLength(
    STATEMENTS_SHOWN,
  );
  expect(dialog()!.textContent).toContain(T.sql.more(3));
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
