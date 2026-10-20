// @vitest-environment happy-dom
// 알림 폭주: 토스트는 합치고 상한을 두어 쌓이지 않고, 기록은 상한(200)까지 남는다. Host 알림(`notification.show`)은 이 창의
// 언어로 만들어 같은 기록에 들어간다.

import { LocalizationProvider } from "@fluent/react";
import { createMemoryTransportPair, RpcClient } from "@pitwall/rpc-client";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { toast, Toaster } from "sonner";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { bundledLocalization, KO_PROJECT } from "../i18n/testing";
import {
  createNotificationStore,
  HOST_NOTIFICATION_TOPIC,
  type NotificationInput,
  NotificationProvider,
  NOTIFICATION_LIMIT,
  type NotificationStore,
  TOAST_LIMIT,
  ToastGate,
  useHostNotifications,
  useNotify,
} from "./notifications";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

let root: Root | undefined;
let notify: (input: NotificationInput) => void = () => {};

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
});

afterEach(async () => {
  // sonner 의 토스트 목록은 모듈 전역이다 — 다음 시험에 남지 않게 닫는다.
  await act(async () => {
    toast.dismiss();
  });
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
});

function Notifier({ client, projectId }: { client?: RpcClient; projectId?: string }) {
  notify = useNotify();
  useHostNotifications(client, projectId);
  return null;
}

async function mount(store: NotificationStore, client?: RpcClient, projectId?: string) {
  const element = document.createElement("div");
  document.body.append(element);
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <LocalizationProvider l10n={bundledLocalization("ko")}>
        <NotificationProvider store={store}>
          <Notifier client={client} projectId={projectId} />
          <Toaster />
        </NotificationProvider>
      </LocalizationProvider>,
    ),
  );
}

const toastNodes = () => [...document.querySelectorAll<HTMLElement>("[data-sonner-toast]")];
const settle = () => act(async () => new Promise((r) => setTimeout(r, 20)));

it("merges the same error into one toast with a count, but records every one", async () => {
  const store = createNotificationStore();
  await mount(store);
  await act(async () => {
    for (let i = 0; i < 100; i++) notify({ level: "error", title: "디스크 없음", body: "gone" });
  });
  await settle();
  expect(toastNodes()).toHaveLength(1);
  expect(toastNodes()[0]!.textContent).toContain("디스크 없음 (×100)");
  expect(store.getState().items).toHaveLength(100);
  expect(store.getState().unseenErrors).toBe(100);
});

it("keeps at most TOAST_LIMIT toasts open and only records the rest", async () => {
  const store = createNotificationStore();
  await mount(store);
  await act(async () => {
    for (let i = 0; i < 100; i++) notify({ level: "error", title: `오류 ${i}` });
  });
  await settle();
  expect(toastNodes().length).toBeLessThanOrEqual(TOAST_LIMIT);
  expect(toastNodes().map((t) => t.textContent)).toEqual(["오류 2", "오류 1", "오류 0"]);
  expect(store.getState().items).toHaveLength(100);
});

it("keeps the newest NOTIFICATION_LIMIT notifications in the record", () => {
  const store = createNotificationStore();
  for (let i = 0; i < 250; i++) store.getState().push({ level: "info", title: `n${i}` });
  const items = store.getState().items;
  expect(items).toHaveLength(NOTIFICATION_LIMIT);
  expect(items[0]!.title).toBe("n249");
  expect(items.at(-1)!.title).toBe("n50");
});

it("frees a toast slot when a toast closes", () => {
  const gate = new ToastGate();
  for (let i = 0; i < TOAST_LIMIT; i++) expect(gate.admit(`k${i}`)).toBe(1);
  expect(gate.admit("over")).toBeUndefined();
  expect(gate.admit("k0")).toBe(2);
  gate.release("k1");
  expect(gate.admit("over")).toBe(1);
});

/** events.subscribe/unsubscribe 만 아는 가짜 Host. `emit` 의 `projectId` 가 `null` 이면 Host 범위다. */
function fakeHost() {
  const [clientSide, serverSide] = createMemoryTransportPair();
  const subscriptions = new Map<number, { projectId?: string; topics: string[] }>();
  let next = 1;
  const send = (message: object) => serverSide.send(JSON.stringify({ jsonrpc: "2.0", ...message }));
  serverSide.onMessage((text) => {
    const message = JSON.parse(text) as { id: number; method: string; params: unknown };
    if (message.method === "events.subscribe") {
      const id = next++;
      subscriptions.set(id, message.params as { projectId?: string; topics: string[] });
      send({ id: message.id, result: { subscriptionId: id } });
    } else if (message.method === "events.unsubscribe") {
      subscriptions.delete((message.params as { subscriptionId: number }).subscriptionId);
      send({ id: message.id, result: null });
    }
  });
  return {
    client: new RpcClient(clientSide),
    subscriptions,
    emit(projectId: string | null, payload: unknown) {
      for (const [subscriptionId, sub] of subscriptions) {
        if ((sub.projectId ?? null) !== projectId || !sub.topics.includes(HOST_NOTIFICATION_TOPIC))
          continue;
        send({
          method: "events.event",
          params: { subscriptionId, projectId, topic: HOST_NOTIFICATION_TOPIC, payload },
        });
      }
    },
  };
}

it("shows host notifications of the host scope and of this window's project in this window's language", async () => {
  const store = createNotificationStore();
  const host = fakeHost();
  await mount(store, host.client, "p1");
  await vi.waitFor(() => expect(host.subscriptions.size).toBe(2));

  await act(async () => {
    host.emit("p1", {
      level: "warning",
      message: "fs-watch-limit",
      args: { root: "/repo", seconds: "5" },
      body: "/repo",
    });
    // 다른 프로젝트의 알림은 이 창에 오지 않는다.
    host.emit("p2", { level: "error", message: "fs-watch-limit", args: { seconds: "5" } });
    host.emit(null, { level: "bogus", message: "runtime-reload-failed" });
  });
  await vi.waitFor(() => expect(store.getState().items).toHaveLength(2));
  const [hostWide, project] = store.getState().items;
  // 모르는 등급은 정보로 띄운다.
  expect(hostWide).toMatchObject({ level: "info", title: KO_PROJECT.runtime.reloadFailed });
  expect(project).toMatchObject({
    level: "warning",
    title: "파일 감시 한도(fs.inotify.max_user_watches)를 넘어 일부 폴더는 5초마다 확인한다",
    body: "/repo",
  });

  act(() => root?.unmount());
  root = undefined;
  await vi.waitFor(() => expect(host.subscriptions.size).toBe(0));
});
