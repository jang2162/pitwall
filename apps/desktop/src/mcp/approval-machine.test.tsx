// @vitest-environment happy-dom
// 승인 다이얼로그 · 「승인 대기」 의 머신 표지(18-m7-plan M7-18 ④): 다른 머신을 겨눈 요청은 대상 머신을 **Desktop 의 별명**으로
// 보이고(`remote.hosts.list` 에서 그 머신 id 를 기억한 항목 — Relay 가 댄 이름이 아니다, M7-P19) [이 세션 동안 <머신> 허용] 을
// 단다. 이름은 요청의 머신 id 로만 찾는다 — 다른 머신의 이름이 붙지 않게(M7-14 B2 의 꼴: 화면의 머신이 실제 대상과 같아야 한다).

import { LocalizationProvider } from "@fluent/react";
import type {
  McpApprovalAnswer,
  McpApprovalEvent,
  McpApprovalRequest,
  RpcClient,
} from "@pitwall/rpc-client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, vi } from "vitest";

import { bundledLocalization, bundledTranslator } from "../i18n/testing";
import { HostConnectionProvider } from "../rpc/host";
import { act, endTest, it } from "../test-support/ended-test";
import type { ApprovalNative } from "./approval-native";
import { createApprovalStore } from "./approvals";
import { WindowApprovals } from "./approvals-host";
import { PendingApprovals } from "./pending-approvals";
import { mcpStrings } from "./strings";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

vi.mock("../keymap/keymap-provider", () => ({ useKeymapPopup: () => undefined }));

const T = mcpStrings(bundledTranslator("ko")).approval;
const MINI = "SHA256:mini";
const BOX = "SHA256:box";

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

/** `mcp.approvals` 스트림 · `mcp.approval.answer` · 원격 호스트 목록(머신 id → 별명)을 답하는 창의 연결. */
function fakeHost() {
  const queue: McpApprovalEvent[] = [];
  let wake: (() => void) | undefined;
  const answers: { id: number; answer: McpApprovalAnswer }[] = [];
  const calls: string[] = [];
  const client = {
    stream(_method: string, _params: unknown, options: { signal: AbortSignal }) {
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
      calls.push(method);
      if (method === "remote.hosts.list")
        return {
          // 별명은 사람이 정한 호스트 항목 이름이다. 두 머신을 두어 엉뚱한 이름이 붙으면 드러난다.
          hosts: [
            { name: "box", machine: BOX },
            { name: "mini", machine: MINI },
          ],
          warnings: [],
        };
      expect(method).toBe("mcp.approval.answer");
      answers.push({ id: params.id, answer: params.answer });
      return null;
    }),
  };
  return {
    client: client as unknown as RpcClient,
    answers,
    calls,
    async push(...events: McpApprovalEvent[]) {
      queue.push(...events);
      await act(async () => {
        wake?.();
        for (let i = 0; i < 50; i++) await Promise.resolve();
      });
    },
  };
}

const native: ApprovalNative = { notify: () => undefined, onReveal: () => () => undefined };

function request(over: Partial<McpApprovalRequest> = {}): McpApprovalRequest {
  return {
    id: 1,
    projectId: "p1",
    machine: MINI,
    notThisProject: false,
    session: "s1",
    client: "claude-code 2.1.0",
    cli: false,
    tool: "run_start",
    tier: "exec",
    plan: {
      method: "run.tool.start",
      tier: "exec",
      summary: "run test",
      changes: [],
      rules: { autoApprove: true, remember: { kind: "tool" }, policyAuto: false },
      hash: "sha256:1",
    },
    answers: ["deny", "once", "machine"],
    timeoutSecs: 300,
    expiresAt: Date.now() + 300_000,
    ...over,
  };
}

async function mount() {
  const host = fakeHost();
  const store = createApprovalStore();
  const element = document.createElement("div");
  document.body.append(element);
  root = createRoot(element);
  const queries = new QueryClient();
  await act(async () =>
    root!.render(
      <QueryClientProvider client={queries}>
        <HostConnectionProvider
          connection={{ target: { hostId: "local" }, key: "host:local", client: host.client }}
        >
          <LocalizationProvider l10n={bundledLocalization("ko")}>
            <WindowApprovals
              client={host.client}
              projectId="p1"
              native={native}
              store={store}
              armDelayMs={0}
            />
            <PendingApprovals projectId="p1" store={store} />
          </LocalizationProvider>
        </HostConnectionProvider>
      </QueryClientProvider>,
    ),
  );
  return host;
}

const dialog = () => document.querySelector<HTMLElement>("[data-testid=mcp-approval-dialog]");
const button = (answer: string) =>
  dialog()?.querySelector<HTMLButtonElement>(`[data-answer=${answer}]`) ?? null;

async function settle() {
  await act(async () => {
    for (let i = 0; i < 50; i++) await Promise.resolve();
  });
}

it("names the target machine by this Desktop's alias and allows that machine for the session", async () => {
  const host = await mount();
  await host.push({ kind: "open", request: request() });
  await settle();
  const machine = dialog()!.querySelector<HTMLElement>("[data-testid=mcp-approval-machine]")!;
  expect(machine.dataset.machine).toBe(MINI);
  await vi.waitFor(() => expect(machine.textContent).toBe("mini"));
  expect(dialog()!.textContent).toContain(T.machineNote);
  expect(button("machine")!.textContent).toBe(T.allowMachine("mini"));
  expect(button("session")).toBeNull();
  const row = document.querySelector("[data-testid=mcp-approval-row-machine]");
  expect(row?.textContent).toBe("mini");
  act(() => button("machine")!.click());
  await settle();
  expect(host.answers).toEqual([{ id: 1, answer: "machine" }]);
});

it("a CLI request allows the machine for the CLI while the window is open; an unknown machine shows its id", async () => {
  const host = await mount();
  await host.push({
    kind: "open",
    request: request({ cli: true, machine: "SHA256:unknown" }),
  });
  await settle();
  expect(dialog()!.querySelector("[data-testid=mcp-approval-machine]")!.textContent).toBe(
    "SHA256:unknown",
  );
  expect(button("machine")!.textContent).toBe(T.allowMachineCli("SHA256:unknown"));
});

it("a request on this computer has no machine line, no machine button and reads no host names", async () => {
  const host = await mount();
  // 이 머신 요청에 `machine` 답이 실려 와도 그리지 않는다(머신이 없으면 무엇을 허용하는지 보일 수 없다).
  await host.push({
    kind: "open",
    request: request({ machine: null, answers: ["deny", "once", "session", "machine"] }),
  });
  await settle();
  expect(dialog()).not.toBeNull();
  expect(dialog()!.querySelector("[data-testid=mcp-approval-machine]")).toBeNull();
  expect(button("machine")).toBeNull();
  expect(button("session")).not.toBeNull();
  expect(document.querySelector("[data-testid=mcp-approval-row-machine]")).toBeNull();
  expect(host.calls).not.toContain("remote.hosts.list");
});
