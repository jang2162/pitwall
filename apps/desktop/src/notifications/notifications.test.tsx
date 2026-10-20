// @vitest-environment happy-dom
// 알림 폭주: 토스트는 합치고 상한을 두어 쌓이지 않고, 기록은 상한(200)까지 남는다.

import { LocalizationProvider } from "@fluent/react";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { toast, Toaster } from "sonner";
import { afterEach, beforeEach, expect, it } from "vitest";

import { bundledLocalization } from "../i18n/testing";
import {
  createNotificationStore,
  type NotificationInput,
  NotificationProvider,
  NOTIFICATION_LIMIT,
  type NotificationStore,
  TOAST_LIMIT,
  ToastGate,
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

function Notifier() {
  notify = useNotify();
  return null;
}

async function mount(store: NotificationStore) {
  const element = document.createElement("div");
  document.body.append(element);
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <LocalizationProvider l10n={bundledLocalization("ko")}>
        <NotificationProvider store={store}>
          <Notifier />
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
