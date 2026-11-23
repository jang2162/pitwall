// @vitest-environment happy-dom
// 「체크아웃 추가」 화면(18-m7-plan M7-11b ① — 완료 기준 ① · ②). StrictMode 로 그린다(test-support/strict-mode.ts).
//
// 가짜 Host 는 실제 `bundle.addMember`(crates/pw-core/src/registry/bundle/methods.rs)의 꼴과 거절 규칙대로 답한다: 모르는 머신은
// not found + `data.reason = "machineUnavailable"`, 모르는 묶음은 invalid params, 판정은 `request.cancel` 이 오면 멈추고(핸들러를
// 버린다 — 답 없음) 시간 상한을 넘기면 cancelled + `data.reason = "judgeTimedOut"`, `seconds`. 파일이 많은 경로는 `allowLarge`
// 없이는 `largeDirectory`.

import { LocalizationProvider } from "@fluent/react";
import {
  createMemoryTransportPair,
  ERROR_CODE,
  type AddBundleMemberParams,
  type ProjectBundle,
  RpcClient,
} from "@pitwall/rpc-client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, vi } from "vitest";

import { act, endTest, it } from "../test-support/ended-test";
import { appStrings } from "../i18n/app-strings";
import { bundledLocalization, bundledTranslator } from "../i18n/testing";
import { AddCheckoutDialog, addOutcomeText, machineChoices } from "./add-checkout";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

vi.mock("../keymap/keymap-provider", () => ({
  useKeymapPopup: () => {},
  useRegisterActions: () => {},
}));

const T = appStrings(bundledTranslator("ko")).checkoutAdd;
const MINI = "SHA256:mini";

const BUNDLE: ProjectBundle = {
  id: "p1",
  name: "web-console",
  favorite: false,
  tags: [],
  openedAt: null,
  implicit: true,
  members: [{ machine: "local", projectId: "p1", lastTarget: null }],
};

interface Fake {
  calls: { id: number; params: AddBundleMemberParams }[];
  cancels: number[];
  /** 판정 중인 요청 — 답을 정하지 않으면 걸려 있다(`request.cancel` 이 오면 지운다 — 실제 Host 는 핸들러를 버린다). */
  judging: Map<number, (answer: object) => void>;
  /** 판정의 답. 없으면 걸어 둔다. */
  judge?: (params: AddBundleMemberParams) => object | undefined;
}

function fakeHost(fake: Fake): RpcClient {
  const [client, server] = createMemoryTransportPair();
  const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
  server.onMessage((text) => {
    const m = JSON.parse(text) as { id?: number; method: string; params: never };
    if (m.method === "request.cancel") {
      const id = (m.params as { id: number }).id;
      fake.cancels.push(id);
      fake.judging.delete(id);
      return;
    }
    if (m.method !== "bundle.addMember")
      return send({ id: m.id, error: { code: ERROR_CODE.METHOD_NOT_FOUND, message: m.method } });
    const params = m.params as AddBundleMemberParams;
    fake.calls.push({ id: m.id!, params });
    if (params.bundleId !== null && params.bundleId !== undefined && params.bundleId !== "p1")
      return send({
        id: m.id,
        error: {
          code: ERROR_CODE.INVALID_PARAMS,
          message: `unknown project \`${params.bundleId}\``,
        },
      });
    if (params.machine !== MINI)
      return send({
        id: m.id,
        error: {
          code: ERROR_CODE.NOT_FOUND,
          message: "machine not connected",
          data: { reason: "machineUnavailable", machine: params.machine },
        },
      });
    const answer = fake.judge?.(params);
    if (answer) return send({ id: m.id, ...answer });
    fake.judging.set(m.id!, (a) => send({ id: m.id, ...a }));
  });
  return new RpcClient(client);
}

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

async function settle() {
  for (let i = 0; i < 5; i++) await act(() => new Promise((r) => setTimeout(r, 0)));
}

async function show(fake: Fake, over: { bundle?: ProjectBundle | undefined } = {}) {
  const added: ProjectBundle[] = [];
  const closed = vi.fn();
  const client = fakeHost(fake);
  const element = document.createElement("div");
  document.body.append(element);
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <LocalizationProvider l10n={bundledLocalization("ko")}>
        <QueryClientProvider client={new QueryClient()}>
          <AddCheckoutDialog
            client={client}
            bundle={"bundle" in over ? over.bundle : BUNDLE}
            machine={MINI}
            onClose={closed}
            onAdded={(b) => added.push(b)}
          />
        </QueryClientProvider>
      </LocalizationProvider>,
    ),
  );
  await settle();
  return { added, closed };
}

const dialog = () => document.querySelector<HTMLElement>("[data-testid=add-checkout-dialog]");
const button = (text: string) =>
  [...(dialog()?.querySelectorAll("button") ?? [])].find((b) => b.textContent === text);
const status = () => document.querySelector("[data-testid=add-checkout-running]")?.textContent;
const result = () => document.querySelector("[data-testid=add-checkout-result]")?.textContent;

async function typePath(path: string) {
  const input = dialog()!.querySelector<HTMLInputElement>("input")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, path);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

async function click(b: HTMLButtonElement | undefined) {
  expect(b).toBeDefined();
  await act(async () => b!.click());
  await settle();
}

const newFake = (judge?: Fake["judge"]): Fake => ({
  calls: [],
  cancels: [],
  judging: new Map(),
  judge,
});

describe("AddCheckoutDialog", () => {
  it("shows progress while adding and its [Cancel] cancels that request on the host", async () => {
    const fake = newFake();
    const { added, closed } = await show(fake);
    await typePath("  /home/me/web-console  ");
    await click(button(T.submit));
    // 머신은 이름이 아니라 머신 id 로, 묶음은 창의 묶음으로 간다. 경로의 앞뒤 공백은 뺀다.
    expect(fake.calls.map((c) => c.params)).toEqual([
      { bundleId: "p1", machine: MINI, path: "/home/me/web-console", allowLarge: null },
    ]);
    expect(status()).toBe(T.running(MINI));
    expect(button(T.submit)).toBeUndefined();
    expect(fake.judging.size).toBe(1);

    await click(button(T.cancel));
    expect(fake.cancels).toEqual([fake.calls[0]!.id]);
    expect(fake.judging.size).toBe(0);
    expect(status()).toBeUndefined();
    expect(result()).toBe(T.cancelled);
    // 대화상자는 남는다 — 다시 할 수 있다. 더해지지도 닫히지도 않았다.
    expect(dialog()).not.toBeNull();
    expect(added).toEqual([]);
    expect(closed).not.toHaveBeenCalled();
  });

  it("puts the time limit refusal in words a person reads", async () => {
    const fake = newFake(() => ({
      error: {
        code: ERROR_CODE.CANCELLED,
        message: "judging stopped after 180 s",
        data: { reason: "judgeTimedOut", seconds: 180 },
      },
    }));
    await show(fake);
    await typePath("/home/me/huge");
    await click(button(T.submit));
    expect(result()).toBe(T.timedOut(180));
    expect(result()).toContain("180초");
    expect(result()).not.toContain("judge");
    expect(result()).not.toContain("(-32");
  });

  it("asks before a large directory and adds it anyway only when told to", async () => {
    const bundle: ProjectBundle = { ...BUNDLE, id: "b-1", implicit: false };
    const fake = newFake((p) =>
      p.allowLarge
        ? { result: { kind: "added", bundle } }
        : { result: { kind: "largeDirectory", atLeast: 200000 } },
    );
    const { added } = await show(fake);
    await typePath("/home/me");
    await click(button(T.submit));
    expect(result()).toBe(T.large(200000));
    await click(button(T.allowLarge));
    expect(fake.calls.map((c) => c.params.allowLarge)).toEqual([null, true]);
    expect(added).toEqual([bundle]);
  });

  it("makes a new remote-only bundle from the remote host sheet", async () => {
    const fake = newFake((p) => ({
      result: {
        kind: "added",
        bundle: {
          ...BUNDLE,
          id: "b-2",
          implicit: false,
          members: [{ machine: p.machine, projectId: "r1", lastTarget: null }],
        },
      },
    }));
    const { added } = await show(fake, { bundle: undefined });
    expect(dialog()!.textContent).toContain(T.newBundle);
    await typePath("/srv/app");
    await click(button(T.submit));
    expect(fake.calls[0]!.params.bundleId).toBeNull();
    expect(added.map((b) => b.id)).toEqual(["b-2"]);
  });
});

describe("addOutcomeText", async () => {
  const name = (m: string) => (m === MINI ? "mini" : m);

  it("words every refusal and names the machine", async () => {
    const refused = (reason: object) =>
      addOutcomeText({ outcome: { kind: "rejected", reason } as never }, T, name);
    expect(refused({ kind: "noCommonCommit", machine: MINI })).toEqual({
      kind: "refused",
      text: T.noCommon("mini"),
    });
    expect(refused({ kind: "shallowUnknown", machine: "local" }).kind).toBe("refused");
    for (const kind of ["notGitRepository", "noCommits"])
      expect(refused({ kind }).kind).toBe("refused");
    expect(refused({ kind: "secondClone", memberProjectId: "x" })).toEqual({
      kind: "refused",
      text: T.secondClone,
    });
  });

  it("tells a timeout, a cancel, an unreachable machine and other errors apart", async () => {
    const { RpcCallError } = await import("@pitwall/rpc-client");
    const err = (code: number, data?: object) =>
      addOutcomeText(
        { error: new RpcCallError({ code, message: "m", data: data as never }) },
        T,
        name,
      );
    expect(err(ERROR_CODE.CANCELLED, { reason: "judgeTimedOut", seconds: 3 }).kind).toBe("refused");
    expect(err(ERROR_CODE.CANCELLED, { reason: "judgeTimedOut", seconds: 3 })).toEqual({
      kind: "refused",
      text: T.timedOut(3),
    });
    expect(err(ERROR_CODE.CANCELLED)).toEqual({ kind: "refused", text: T.cancelled });
    expect(err(ERROR_CODE.NOT_FOUND, { reason: "machineUnavailable" })).toEqual({
      kind: "refused",
      text: T.unavailable,
    });
    expect(err(ERROR_CODE.INVALID_PARAMS)).toEqual({
      kind: "refused",
      text: T.failed("m (-32602)"),
    });
  });
});

describe("machineChoices", () => {
  it("offers only machines with a known id that are not in the bundle yet, once each", async () => {
    expect(
      machineChoices(
        [
          { name: "mini", machine: MINI },
          { name: "mini-again", machine: MINI },
          { name: "never-connected", machine: null },
          { name: "pc", machine: "SHA256:pc" },
          { name: "self", machine: "local" },
        ],
        new Set(["SHA256:pc"]),
      ),
    ).toEqual([{ machine: MINI, name: "mini" }]);
  });
});
