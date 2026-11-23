// @vitest-environment happy-dom
// 웰컴 창의 「원격 호스트」 탭(18-m7-plan M7-08 웹뷰 시험: 단계별 실패 표시 · 다시 시도). 앱(main.tsx)처럼 StrictMode 로 렌더한다 —
// `remote.connect` 스트림은 붙였다 떼고 다시 붙여도 하나만 남아야 한다. 가짜 Host 가 스트림에 상태를 보낸다.

import type {
  RemoteForward,
  ConfigView,
  HostInfo,
  KeymapFile,
  RemoteHostInfo,
  RelayDevice,
  RelayListen,
  RemoteOldRelay,
  RemoteRelays,
  RemoteStatus,
  RemoteStopOutcome,
} from "@pitwall/rpc-client";
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import type { KeymapNative } from "../keymap/native";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { stageMarks } from "./remote-hosts";
import type { WelcomeNative } from "./native";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const host = vi.hoisted(() => ({
  hosts: [] as RemoteHostInfo[],
  calls: [] as { method: string; params: unknown }[],
  /** 열린 `remote.connect` 스트림(이름 → 보내기). 취소되면 빠진다. */
  connects: new Map<number, { name: string; send: (s: RemoteStatus) => void }>(),
  prompt: (_event: unknown) => {},
  /** 이 Host 가 그 링크에서 본 원격 Relay(`remote.relays`). */
  relays: null as RemoteRelays | null,
  /** 원격의 지금 옛 판 데몬(사람이 본 뒤 바뀔 수 있다 — 실제 Host 처럼 `remote.stopOld` 가 이것과 견준다). */
  remoteOld: [] as RemoteOldRelay[],
  /** 이 Host 의 포트 포워딩(`remote.forwards.list` — 18-m7-plan M7-22). */
  forwards: [] as RemoteForward[],
  /** 그 Relay 의 페어링 표(`remote.devices`). */
  devices: [] as RelayDevice[],
  /** `remote.pairCode` 가 돌려줄 만료 시각(없으면 5분 뒤). */
  codeExpiresAt: undefined as number | undefined,
  /** 그 Relay 의 직접 연결 듣기(`remote.devices` 의 `listen` — 18-m7-plan M7-24). */
  listen: null as RelayListen | null,
  /** 그 머신의 Relay 가 상주인가(`remote.residentStatus`). */
  resident: false,
}));

vi.mock("../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../keymap/default-keymap");
  const { i18nState } = await import("../i18n/testing");
  return {
    connectHost: async () => {
      const [client, server] = pair();
      let next = 1;
      const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const reply = (result: unknown) => send({ id: m.id, result });
        host.calls.push({ method: m.method, params: m.params });
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
          case "events.subscribe":
            return reply({ subscriptionId: next++ });
          case "events.unsubscribe":
            return reply(null);
          case "project.list":
          case "bundle.list":
            return reply([]);
          case "git.projectBranches":
            return reply({ projects: [] });
          case "project.storage":
            return reply({ kind: "ok", path: "/d/state.db" });
          case "remote.hosts.list":
            return reply({ hosts: host.hosts, warnings: [] });
          case "remote.forwards.list": {
            const { name } = m.params as { name: string | null };
            return reply(host.forwards.filter((f) => name === null || f.name === name));
          }
          case "remote.forward.close": {
            const { id } = m.params as { id: number };
            host.forwards = host.forwards.filter((f) => f.id !== id);
            return reply(null);
          }
          case "remote.log":
            return reply([
              {
                at: 1,
                stage: "ssh",
                level: "error",
                text: "ssh failed: Permission denied (publickey).",
              },
            ]);
          case "remote.disconnect":
          case "remote.retry":
          case "remote.prompt.answer":
            return reply(null);
          case "remote.relays":
            return reply(host.relays);
          case "remote.devices":
          case "remote.unpairDevice":
          case "remote.pairCode": {
            // 실제 Host 의 규칙: 그 호스트가 지금 사람이 본 머신에 붙어 있을 때만.
            const p = m.params as { machine: string; device?: string };
            if (p.machine !== "SHA256:abc")
              return send({
                id: m.id,
                error: { code: -32602, message: "not connected to that machine" },
              });
            if (m.method === "remote.devices")
              return reply({ devices: host.devices, listen: host.listen });
            if (m.method === "remote.unpairDevice") {
              const before = host.devices.length;
              host.devices = host.devices.filter((d) => d.device !== p.device);
              return reply({ removed: host.devices.length !== before });
            }
            return reply({
              code: "042917",
              expiresAtMs: host.codeExpiresAt ?? Date.now() + 300_000,
            });
          }
          case "remote.stopOld":
          case "remote.linkCommand": {
            // 실제 Host 의 규칙: 그 호스트가 지금 사람이 본 머신에 붙어 있을 때만(`RemoteHosts::linked_to`).
            const p = m.params as { machine: string; seen?: RemoteOldRelay; on?: boolean };
            if (!host.relays || p.machine !== host.relays.machine)
              return send({
                id: m.id,
                error: { code: -32602, message: "not connected to that machine" },
              });
            if (m.method === "remote.linkCommand") {
              host.relays = { ...host.relays, command: p.on ? "linked" : "absent" };
              return reply(null);
            }
            const now = host.remoteOld.find((o) => o.file === p.seen!.file);
            let outcome: RemoteStopOutcome;
            if (!now) outcome = { outcome: "gone" };
            else if (JSON.stringify(now) !== JSON.stringify(p.seen))
              outcome = { outcome: "changed", now };
            else {
              outcome = { outcome: "stopped" };
              host.remoteOld = host.remoteOld.filter((o) => o !== now);
            }
            host.relays = { ...host.relays, old: host.remoteOld };
            return reply(outcome);
          }
          case "remote.discover":
            return reply({
              found: [
                {
                  source: "tailscale",
                  label: "mini",
                  address: "mini.tail1234.ts.net",
                  port: 7420,
                  listening: true,
                },
                {
                  source: "lan",
                  label: "192.168.1.30",
                  address: "192.168.1.30",
                  port: 7500,
                  listening: false,
                },
              ],
              notes: [],
            });
          case "remote.pairDirect": {
            const p = m.params as { code: string; nickname: string };
            if (p.code !== "246810")
              return send({
                id: m.id,
                error: { code: -32000, message: "the relay refused the code" },
              });
            return reply({ name: p.nickname, machine: "SHA256:box", role: "owner" });
          }
          case "remote.residentStatus":
            return reply({ resident: host.resident });
          case "remote.resident": {
            host.resident = (m.params as { on: boolean }).on;
            return reply({ resident: host.resident });
          }
          case "remote.prompts": {
            const streamId = next++;
            reply({ streamId });
            host.prompt = (event) =>
              send({ method: "stream.chunk", params: { streamId, data: event } });
            return;
          }
          case "remote.connect": {
            const streamId = next++;
            reply({ streamId });
            host.connects.set(streamId, {
              name: (m.params as { name: string }).name,
              send: (data) => send({ method: "stream.chunk", params: { streamId, data } }),
            });
            return;
          }
          case "stream.cancel":
            host.connects.delete((m.params as { streamId: number }).streamId);
            return;
        }
      });
      return { transport: client, close: async () => {} };
    },
  };
});

const keymapNative = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: vi.fn<KeymapNative["openSettings"]>(async () => {}),
  closeWindow: vi.fn<KeymapNative["closeWindow"]>(async () => {}),
} satisfies KeymapNative;

const native = {
  openProjectWindow: vi.fn<WelcomeNative["openProjectWindow"]>(async () => {}),
  openFileWindow: vi.fn<WelcomeNative["openFileWindow"]>(async () => {}),
  takePendingFindProject: vi.fn<WelcomeNative["takePendingFindProject"]>(async () => null),
  onFindProjectRequested: vi.fn<WelcomeNative["onFindProjectRequested"]>(() => () => {}),
  pickFolder: vi.fn<WelcomeNative["pickFolder"]>(async () => null),
  reveal: vi.fn<WelcomeNative["reveal"]>(async () => {}),
  onDragDrop: vi.fn<WelcomeNative["onDragDrop"]>(() => () => {}),
  pathKind: vi.fn<WelcomeNative["pathKind"]>(async () => "dir"),
} satisfies WelcomeNative;

let root: Root | undefined;

function mini(
  status: RemoteStatus = { state: "disconnected", byUser: false, error: null },
): RemoteHostInfo {
  return {
    name: "mini",
    host: "mini.lan",
    user: "dev",
    port: null,
    source: "sshConfig",
    machine: null,
    status,
    lastLink: null,
    via: null,
    directPort: 7420,
    ssh: true,
  };
}

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  host.hosts = [mini()];
  host.calls = [];
  host.connects.clear();
  host.relays = null;
  host.remoteOld = [];
  host.forwards = [];
  host.devices = [];
  host.codeExpiresAt = undefined;
  host.listen = null;
  host.resident = false;
});

afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 20)));

async function mount() {
  const element = document.createElement("div");
  document.body.append(element);
  const store = createThemeStore({
    system: { current: () => "light", subscribe: () => () => {} },
    storage: { read: () => undefined, write: () => {} },
    theme: parseTheme(defaultTheme),
    apply: () => {},
  });
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <StrictMode>
        <ThemeStoreProvider store={store}>
          <App kind="welcome" native={keymapNative} welcomeNative={native} />
        </ThemeStoreProvider>
      </StrictMode>,
    ),
  );
  await settle();
}

async function click(el: Element) {
  await act(async () => (el as HTMLElement).click());
  await settle();
}

const button = (text: string) =>
  [...document.querySelectorAll("button")].find((b) => b.textContent === text);

async function openRemoteTab() {
  await mount();
  await click(button("원격 호스트")!);
  await vi.waitFor(() => expect(document.querySelector("[data-remote-host=mini]")).not.toBeNull());
}

function stageMark(stage: string) {
  return document.querySelector(`[data-stage=${stage}]`)?.getAttribute("data-mark");
}

async function push(status: RemoteStatus) {
  await act(async () => {
    for (const c of host.connects.values()) c.send(status);
  });
  await settle();
}

it("connects through one remote.connect stream, shows each stage and the stage that failed, and retries", async () => {
  await openRemoteTab();
  await click(button("연결")!);
  await vi.waitFor(() => expect(host.connects.size).toBe(1));
  expect([...host.connects.values()][0]!.name).toBe("mini");

  await push({ state: "connecting", stage: "install" });
  expect(stageMark("ssh")).toBe("done");
  expect(stageMark("install")).toBe("active");
  expect(stageMark("daemon")).toBe("pending");
  expect(document.body.textContent).toContain("붙는 중 — 설치");

  await push({
    state: "disconnected",
    byUser: false,
    error: { stage: "daemon", kind: "link", message: "the link did not come up" },
  });
  expect(stageMark("install")).toBe("done");
  expect(stageMark("daemon")).toBe("failed");
  expect(document.querySelector("[data-testid=remote-status]")!.textContent).toContain(
    "데몬 단계에서 실패",
  );

  await click(button("다시 시도")!);
  await vi.waitFor(() =>
    expect(host.calls.filter((c) => c.method === "remote.retry")).toHaveLength(1),
  );
  expect(host.calls.find((c) => c.method === "remote.retry")!.params).toEqual({ name: "mini" });

  await push({ state: "connected", machine: "SHA256:abc" });
  expect(stageMark("handshake")).toBe("done");
  await click(button("끊기")!);
  await vi.waitFor(() =>
    expect(host.calls.filter((c) => c.method === "remote.disconnect")).toHaveLength(1),
  );

  // 연결 로그는 원문 그대로 보인다.
  await click(button("연결 로그")!);
  await vi.waitFor(() =>
    expect(document.querySelector("[data-testid=remote-log]")?.textContent).toContain(
      "Permission denied (publickey).",
    ),
  );
});

it("releases the host when the window leaves the remote tab", async () => {
  await openRemoteTab();
  await click(button("연결")!);
  await vi.waitFor(() => expect(host.connects.size).toBe(1));
  await click(button("프로젝트")!);
  await vi.waitFor(() => expect(host.connects.size).toBe(0));
});

it("offers only trusting the new machine when another machine answered", async () => {
  host.hosts = [mini({ state: "machineChanged", expected: "SHA256:old", found: "SHA256:new" })];
  await openRemoteTab();
  expect(document.body.textContent).toContain("SHA256:old");
  expect(button("연결")).toBeUndefined();
  await click(button("이 머신을 새로 믿기")!);
  await vi.waitFor(() =>
    expect(host.calls.find((c) => c.method === "remote.trustMachine")?.params).toEqual({
      name: "mini",
      machine: "SHA256:new",
    }),
  );
});

it("asks the remote ssh host key question with 취소 focused first and answers yes", async () => {
  await mount();
  await act(async () =>
    host.prompt({
      kind: "open",
      prompt: {
        promptId: 3,
        request: {
          kind: "askpass",
          text: "The authenticity of host 'mini (192.0.2.7)' can't be established.\nED25519 key fingerprint is SHA256:fp.\nAre you sure you want to continue connecting (yes/no/[fingerprint])?",
          hint: "secret",
        },
      },
    }),
  );
  const dialog = await vi.waitFor(() => {
    const el = document.querySelector("[data-testid=git-askpass-dialog]");
    if (!el) throw new Error("no dialog");
    return el;
  });
  expect(document.activeElement?.textContent).toBe("취소");
  const yes = [...dialog.querySelectorAll("button")].find((b) => b.textContent === "예")!;
  await click(yes);
  await vi.waitFor(() =>
    expect(host.calls.find((c) => c.method === "remote.prompt.answer")?.params).toEqual({
      promptId: 3,
      answer: { kind: "askpass", value: "yes" },
    }),
  );
});

it("marks the stages from the status", () => {
  expect(stageMarks({ state: "connected", machine: "m" })).toEqual({
    direct: "done",
    ssh: "done",
    install: "done",
    daemon: "done",
    handshake: "done",
  });
  expect(
    stageMarks({
      state: "reconnecting",
      attempt: 2,
      retryInMs: 2000,
      error: { stage: "ssh", kind: "ssh", message: "x" },
    }).ssh,
  ).toBe("failed");
});

const OLD: RemoteOldRelay = {
  file: "pitwall-relay-dev-0.9.0",
  version: "0.9.0",
  channel: "dev",
  pid: 4242,
  runs: 1,
  terminals: 2,
  links: 0,
};

function relaysOf(old: RemoteOldRelay[]): RemoteRelays {
  return {
    machine: "SHA256:abc",
    old,
    silent: [{ file: "pitwall-relay-dev-0.8.0", version: "0.8.0" }],
    command: "absent",
    removed: [],
  };
}

async function connectMini() {
  await openRemoteTab();
  await click(button("연결")!);
  await vi.waitFor(() => expect(host.connects.size).toBe(1));
  await push({ state: "connected", machine: "SHA256:abc" });
  await vi.waitFor(() =>
    expect(document.querySelector("[data-testid=remote-relays]")).not.toBeNull(),
  );
}

const stops = () => host.calls.filter((c) => c.method === "remote.stopOld").map((c) => c.params);

it("shows the old relay daemons of the link and stops one with exactly the values seen", async () => {
  const shared: RemoteOldRelay = {
    ...OLD,
    file: "pitwall-relay-dev-0.9.5",
    version: "0.9.5",
    links: 2,
  };
  host.remoteOld = [OLD, shared];
  host.relays = relaysOf(host.remoteOld);
  await connectMini();
  const text = document.querySelector("[data-testid=remote-relays]")!.textContent;
  expect(text).toContain("옛 판 Relay 0.9.0 에 실행 1개 · 터미널 2개가 남았다.");
  expect(text).toContain("옛 판 Relay 0.8.0 이 답하지 않는다");
  // 다른 Desktop 이 붙은 옛 판은 그 수와 「그 연결도 끊긴다」, 단추 글도 다르다.
  const sharedRow = document.querySelector(`[data-old-relay="${shared.file}"]`)!;
  expect(sharedRow.textContent).toContain("다른 Desktop 연결 2개가 이 옛 판에 붙어 있다");
  expect(button("옛 판 끝내기 (연결 2개도 끊김)")).toBeDefined();
  // 답하지 않는 옛 판에는 끝내기 단추가 없다.
  expect(document.querySelector("[data-silent-relay] button")).toBeNull();

  const row = document.querySelector(`[data-old-relay="${OLD.file}"]`)!;
  await click(row.querySelector("button")!);
  await vi.waitFor(() => expect(stops()).toHaveLength(1));
  expect(stops()[0]).toEqual({ name: "mini", machine: "SHA256:abc", seen: OLD });
  await vi.waitFor(() => expect(document.body.textContent).toContain("옛 판 Relay 를 끝냈다."));
  await vi.waitFor(() =>
    expect(document.querySelector(`[data-old-relay="${OLD.file}"]`)).toBeNull(),
  );

  // 다른 Desktop 이 붙은 옛 판도 보인 값(붙은 링크 수 포함) 그대로 싣는다.
  await click(button("옛 판 끝내기 (연결 2개도 끊김)")!);
  await vi.waitFor(() => expect(stops()).toHaveLength(2));
  expect(stops()[1]).toEqual({ name: "mini", machine: "SHA256:abc", seen: shared });
});

it("does not stop an old daemon that changed after it was seen and asks again with the new counts", async () => {
  host.remoteOld = [OLD];
  host.relays = relaysOf(host.remoteOld);
  await connectMini();
  // 사람이 본 뒤 원격에서 터미널이 하나 늘었다(화면은 아직 옛 수).
  const grown = { ...OLD, terminals: 3 };
  host.remoteOld = [grown];
  await click(button("옛 판 끝내기")!);
  await vi.waitFor(() => expect(stops()).toHaveLength(1));
  expect(stops()[0]).toEqual({ name: "mini", machine: "SHA256:abc", seen: OLD });
  await vi.waitFor(() =>
    expect(document.body.textContent).toContain("본 뒤에 바뀌어 끝내지 않았다"),
  );
  await vi.waitFor(() => expect(document.body.textContent).toContain("터미널 3개가 남았다"));
  await click(button("옛 판 끝내기")!);
  await vi.waitFor(() => expect(stops()).toHaveLength(2));
  expect(stops()[1]).toEqual({ name: "mini", machine: "SHA256:abc", seen: grown });
  await vi.waitFor(() => expect(document.body.textContent).toContain("옛 판 Relay 를 끝냈다."));
});

it("links the pitwall command only when the person ticks it, on the machine it surveyed", async () => {
  host.relays = relaysOf([]);
  await connectMini();
  expect(host.calls.some((c) => c.method === "remote.linkCommand")).toBe(false);
  const box = document.querySelector<HTMLInputElement>(
    "[data-testid=remote-relays] input[type=checkbox]",
  )!;
  expect(box.checked).toBe(false);
  await click(box);
  await vi.waitFor(() =>
    expect(host.calls.find((c) => c.method === "remote.linkCommand")?.params).toEqual({
      name: "mini",
      machine: "SHA256:abc",
      on: true,
    }),
  );
  await vi.waitFor(() =>
    expect(
      document.querySelector<HTMLInputElement>("[data-testid=remote-relays] input[type=checkbox]")!
        .checked,
    ).toBe(true),
  );
});

it("says under 끊기 that the relay still holds the machine's runs while reconnecting", async () => {
  await openRemoteTab();
  await click(button("연결")!);
  await vi.waitFor(() => expect(host.connects.size).toBe(1));
  await push({ state: "connected", machine: "SHA256:abc" });
  expect(document.body.textContent).not.toContain("이어 붙기 시한");
  await push({
    state: "reconnecting",
    attempt: 1,
    retryInMs: 1000,
    error: { stage: "handshake", kind: "link", message: "x" },
  });
  expect(button("끊기")).toBeDefined();
  expect(document.body.textContent).toContain(
    "끊어도 Relay 가 이어 붙기 시한(2분) 동안 그 머신의 실행 · 터미널을 쥐고 있다.",
  );
});

it("locks the pitwall command box and says why when another file sits there", async () => {
  host.relays = { ...relaysOf([]), command: "other" };
  await connectMini();
  const box = document.querySelector<HTMLInputElement>(
    "[data-testid=remote-relays] input[type=checkbox]",
  )!;
  expect(box.disabled).toBe(true);
  expect(document.body.textContent).toContain(
    "~/.local/bin/pitwall 에 다른 파일이 있어 두지 않는다.",
  );
  await click(box);
  expect(host.calls.some((c) => c.method === "remote.linkCommand")).toBe(false);
});

it("shows the old relay versions only while the host is connected", async () => {
  host.remoteOld = [OLD];
  host.relays = relaysOf(host.remoteOld);
  await openRemoteTab();
  await click(button("연결")!);
  await vi.waitFor(() => expect(host.connects.size).toBe(1));
  for (const status of [
    { state: "connecting", stage: "daemon" },
    {
      state: "reconnecting",
      attempt: 1,
      retryInMs: 1000,
      error: { stage: "handshake", kind: "link", message: "x" },
    },
  ] satisfies RemoteStatus[]) {
    await push(status);
    expect(document.querySelector("[data-testid=remote-relays]")).toBeNull();
    expect(button("옛 판 끝내기")).toBeUndefined();
  }
  await push({ state: "connected", machine: "SHA256:abc" });
  await vi.waitFor(() =>
    expect(document.querySelector("[data-testid=remote-relays]")).not.toBeNull(),
  );
});

// 18-m7-plan M7-14 리뷰 F2: 링크가 이어 붙는 동안(`resuming`)에도 [끊기] 가 있다 — 이 행 전에는 그 동안 상태가 `connected` 였다.
it("keeps Disconnect while the link resumes", async () => {
  await openRemoteTab();
  await click(button("연결")!);
  await vi.waitFor(() => expect(host.connects.size).toBe(1));
  await push({ state: "resuming", machine: "SHA256:abc" });
  expect(document.querySelector("[data-testid=remote-status]")!.textContent).toContain(
    "이어 붙는 중",
  );
  // [끊기] 아래 안내(M7-07b) — 이어 붙는 동안 끊어도 Relay 가 시한 동안 쥔다.
  expect(document.body.textContent).toContain("이어 붙기 시한(2분)");
  await click(button("끊기")!);
  await vi.waitFor(() =>
    expect(host.calls.filter((c) => c.method === "remote.disconnect")).toHaveLength(1),
  );
});

// 18-m7-plan M7-22 ⑤: 호스트 줄에 그 호스트의 포트 포워딩과 [끄기]. 열지 못한 설정의 포워딩(루프백이 아닌 local_host)은 그 까닭을
// 보인다.
it("lists the host's port forwards with the reason a setting was not opened and turns one off", async () => {
  host.forwards = [
    {
      id: 1,
      name: "mini",
      origin: "settings",
      localHost: "127.0.0.1",
      localPort: 8080,
      remoteHost: "localhost",
      remotePort: 8080,
      state: { state: "listening" },
    },
    {
      id: 2,
      name: "mini",
      origin: "settings",
      localHost: "0.0.0.0",
      localPort: 5005,
      remoteHost: "localhost",
      remotePort: 5005,
      state: {
        state: "failed",
        error: { kind: "notLoopback", message: "local_host is not a loopback address" },
      },
    },
  ];
  await openRemoteTab();
  const rows = await vi.waitFor(() => {
    const r = [...document.querySelectorAll<HTMLElement>("[data-testid=remote-forward]")];
    expect(r).toHaveLength(2);
    return r;
  });
  expect(rows[0]!.textContent).toContain("127.0.0.1:8080");
  expect(rows[0]!.textContent).toContain("localhost:8080");
  expect(rows[1]!.dataset.state).toBe("failed");
  expect(rows[1]!.querySelector("[data-testid=remote-forward-why]")?.textContent).toContain(
    "0.0.0.0",
  );
  await click(rows[0]!.querySelector("[data-testid=remote-forward-off]")!);
  expect(host.calls.find((c) => c.method === "remote.forward.close")?.params).toEqual({ id: 1 });
});

function device(over: Partial<RelayDevice>): RelayDevice {
  return {
    device: "SHA256:me",
    key: "AAAA",
    name: "mac",
    role: "owner",
    pairedAtMs: 1_700_000_000_000,
    lastSeenMs: null,
    you: false,
    ...over,
  };
}

async function openDevices() {
  host.relays = relaysOf([]);
  await connectMini();
  await click(button("기기")!);
  await vi.waitFor(() =>
    expect(document.querySelector("[data-testid=remote-devices] [data-device]")).not.toBeNull(),
  );
}

const unpairs = () =>
  host.calls.filter((c) => c.method === "remote.unpairDevice").map((c) => c.params);

it("lists the relay's devices with role and times and unpairs one after asking, on the machine seen", async () => {
  host.devices = [
    device({ device: "SHA256:me", you: true }),
    device({ device: "SHA256:tv", name: "tv", role: "readonly", lastSeenMs: 1_700_000_100_000 }),
  ];
  await openDevices();
  const panel = document.querySelector("[data-testid=remote-devices]")!;
  expect(host.calls.find((c) => c.method === "remote.devices")?.params).toEqual({
    name: "mini",
    machine: "SHA256:abc",
  });
  const tv = panel.querySelector('[data-device="SHA256:tv"]')!;
  expect(tv.textContent).toContain("readonly — 조회만");
  expect(tv.textContent).toContain("마지막 접속");
  expect(panel.querySelector('[data-device="SHA256:me"]')!.textContent).toContain("이 Desktop");
  expect(panel.querySelector('[data-device="SHA256:me"]')!.textContent).toContain(
    "마지막 접속 없음",
  );

  await click(tv.querySelector("button")!);
  expect(unpairs()).toHaveLength(0);
  expect(document.querySelector("[role=alertdialog]")!.textContent).toContain(
    "tv 을 해제하면 그 기기의 연결 · 터미널이 곧바로 끊기고",
  );
  await click(button("해제한다")!);
  await vi.waitFor(() => expect(unpairs()).toHaveLength(1));
  expect(unpairs()[0]).toEqual({ name: "mini", machine: "SHA256:abc", device: "SHA256:tv" });
  await vi.waitFor(() => expect(document.querySelector('[data-device="SHA256:tv"]')).toBeNull());
});

it("warns before this desktop unpairs itself as the last owner but does not block it", async () => {
  host.devices = [
    device({ you: true }),
    device({ device: "SHA256:tv", name: "tv", role: "readonly" }),
  ];
  await openDevices();
  const me = document.querySelector('[data-device="SHA256:me"]')!;
  await click(me.querySelector("button")!);
  const ask = document.querySelector("[role=alertdialog]")!;
  expect(ask.textContent).toContain("마지막 owner 기기다");
  expect(ask.textContent).toContain("SSH 로 다시 페어링");
  // [취소] 가 처음 포커스다.
  expect(document.activeElement?.textContent).toBe("취소");
  await click(button("해제한다")!);
  await vi.waitFor(() => expect(unpairs()).toHaveLength(1));
});

it("shows a pairing code for the chosen role on screen only", async () => {
  const logs = [
    vi.spyOn(console, "log"),
    vi.spyOn(console, "info"),
    vi.spyOn(console, "warn"),
    vi.spyOn(console, "debug"),
  ];
  host.devices = [device({ you: true })];
  await openDevices();
  const select = document.querySelector<HTMLSelectElement>("[data-testid=remote-devices] select")!;
  await act(async () => {
    select.value = "mobile";
    select.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await click(button("코드 띄우기")!);
  await vi.waitFor(() =>
    expect(document.querySelector("[data-testid=pairing-code]")?.textContent).toContain("042 917"),
  );
  expect(host.calls.find((c) => c.method === "remote.pairCode")?.params).toEqual({
    name: "mini",
    machine: "SHA256:abc",
    role: "mobile",
    deviceName: null,
  });
  for (const spy of logs)
    for (const args of spy.mock.calls) expect(JSON.stringify(args)).not.toContain("042917");
});

it("takes an expired pairing code off the screen", async () => {
  host.devices = [device({ you: true })];
  await openDevices();
  host.codeExpiresAt = Date.now() + 400;
  await click(button("코드 띄우기")!);
  await vi.waitFor(() =>
    expect(document.querySelector("[data-testid=pairing-code]")?.textContent).toContain("042 917"),
  );
  await vi.waitFor(() => expect(document.querySelector("[data-testid=pairing-code]")).toBeNull());
});

// ── 직접 연결(18-m7-plan M7-24) ──────────────────────────────────────────────────────────────────────────────

const pairs = () => host.calls.filter((c) => c.method === "remote.pairDirect").map((c) => c.params);

async function type(input: HTMLInputElement, value: string) {
  await act(async () => {
    const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    set.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();
}

function field(label: string): HTMLInputElement {
  const found = [...document.querySelectorAll("[data-testid=remote-direct] label")].find((l) =>
    l.textContent?.startsWith(label),
  );
  return found!.querySelector("input")!;
}

it("pairs a manually placed relay with an address picked from the search and a 6-digit code", async () => {
  await openRemoteTab();
  await click(button("코드로 Relay 붙이기…")!);
  await click(button("검색")!);
  const picked = document.querySelector('[data-discovered="mini.tail1234.ts.net"]')!;
  expect(picked.textContent).toContain("Tailscale");
  expect(picked.textContent).toContain("Relay 가 받는 중");
  expect(document.querySelector('[data-discovered="192.168.1.30"]')!.textContent).toContain(
    "응답 없음",
  );
  await click(picked);
  expect(field("주소").value).toBe("mini.tail1234.ts.net");
  // 이미 있는 이름(mini)은 고를 수 없다 — 다른 이름을 준다.
  expect(button("페어링")!.disabled).toBe(true);
  await type(field("이 호스트의 이름"), "mini-direct");
  await type(field("코드"), "135 79");
  expect(button("페어링")!.disabled).toBe(true);
  // 틀린 코드면 Host 의 거절을 보인다(코드는 글에 없다).
  await type(field("코드"), "135 791");
  await click(button("페어링")!);
  await vi.waitFor(() =>
    expect(document.querySelector("[data-testid=remote-direct] [role=alert]")).not.toBeNull(),
  );
  expect(
    document.querySelector("[data-testid=remote-direct] [role=alert]")!.textContent,
  ).not.toContain("135791");
  await type(field("코드"), "246 810");
  await click(button("페어링")!);
  await vi.waitFor(() => expect(pairs()).toHaveLength(2));
  expect(pairs()[1]).toEqual({
    address: "mini.tail1234.ts.net",
    port: null,
    code: "246810",
    nickname: "mini-direct",
  });
  await vi.waitFor(() =>
    expect(
      document.querySelector("[data-testid=remote-direct] [role=status]")!.textContent,
    ).toContain("mini-direct 을 더했다"),
  );
  // 코드는 칸에서 지운다.
  expect(field("코드").value).toBe("");
});

it("says whether the link stands directly or over ssh", async () => {
  host.hosts = [{ ...mini(), via: "direct" }];
  host.relays = relaysOf([]);
  await connectMini();
  const via = document.querySelector("[data-testid=remote-via]")!;
  expect(via.getAttribute("data-via")).toBe("direct");
  expect(via.textContent).toBe("직접 연결(QUIC)");
});

it("makes the relay resident only when the person ticks it, and really turns it off", async () => {
  host.relays = relaysOf([]);
  await connectMini();
  const box = await vi.waitFor(() => {
    const el = document.querySelector<HTMLInputElement>("[data-testid=remote-resident]");
    expect(el).not.toBeNull();
    return el!;
  });
  expect(box.checked).toBe(false);
  expect(host.calls.some((c) => c.method === "remote.resident")).toBe(false);
  await click(box);
  await vi.waitFor(() => expect(host.resident).toBe(true));
  expect(host.calls.find((c) => c.method === "remote.resident")?.params).toEqual({
    name: "mini",
    machine: "SHA256:abc",
    on: true,
  });
  await vi.waitFor(() =>
    expect(document.querySelector<HTMLInputElement>("[data-testid=remote-resident]")!.checked).toBe(
      true,
    ),
  );
  await click(document.querySelector("[data-testid=remote-resident]")!);
  await vi.waitFor(() => expect(host.resident).toBe(false));
});

it("has no resident box for a direct-only host", async () => {
  host.hosts = [{ ...mini(), ssh: false }];
  host.relays = relaysOf([]);
  await connectMini();
  expect(document.querySelector("[data-testid=remote-resident]")).toBeNull();
});

it("warns in the device section when the relay listens on every interface", async () => {
  host.devices = [device({ you: true })];
  host.listen = { port: 7420, interfaces: ["all"], wide: true, addresses: ["203.0.113.7:7420"] };
  await openDevices();
  expect(document.querySelector("[data-testid=relay-listen-wide]")!.textContent).toContain(
    "모든 인터페이스(공인 주소 포함)",
  );
  expect(document.querySelector("[data-testid=relay-listen]")!.textContent).toContain(
    "203.0.113.7:7420",
  );
});
