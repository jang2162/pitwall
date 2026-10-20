// @vitest-environment happy-dom
// 「환경 다시 읽기」: 성공하면 조용하고, 실패하면 알림(오류)이 하나 생긴다.

import { LocalizationProvider } from "@fluent/react";
import type { Action } from "@pitwall/keymap";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { toast } from "sonner";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { bundledLocalization } from "../i18n/testing";
import { useProjectStrings } from "../layout/strings";
import {
  createNotificationStore,
  NotificationProvider,
  type NotificationStore,
  useNotify,
} from "../notifications/notifications";
import { runtimeActions } from "./runtime-actions";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

let root: Root | undefined;
let action: Action | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
});

afterEach(async () => {
  await act(async () => {
    toast.dismiss();
  });
  act(() => root?.unmount());
  root = undefined;
  action = undefined;
  document.body.innerHTML = "";
});

function Actions({ reload }: { reload(): Promise<void> }) {
  const strings = useProjectStrings().runtime;
  const notify = useNotify();
  action = runtimeActions({ strings, reload, notify })[0];
  return null;
}

async function mount(store: NotificationStore, reload: () => Promise<void>) {
  const element = document.createElement("div");
  document.body.append(element);
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <LocalizationProvider l10n={bundledLocalization("ko")}>
        <NotificationProvider store={store}>
          <Actions reload={reload} />
        </NotificationProvider>
      </LocalizationProvider>,
    ),
  );
}

it("reloads the runtime environment without a notification", async () => {
  const store = createNotificationStore();
  const reload = vi.fn(async () => {});
  await mount(store, reload);
  expect(action?.id).toBe("runtime.reload");
  expect(action?.label).toBe("런타임 환경 다시 읽기");
  await act(async () => action!.run());
  expect(reload).toHaveBeenCalledTimes(1);
  expect(store.getState().items).toEqual([]);
});

it("notifies one error when the reload fails", async () => {
  const store = createNotificationStore();
  await mount(store, async () => {
    throw new Error("storage unavailable");
  });
  await act(async () => action!.run());
  expect(store.getState().items).toMatchObject([
    { level: "error", title: "런타임 환경을 다시 읽지 못했다", body: "storage unavailable" },
  ]);
  expect(store.getState().unseenErrors).toBe(1);
});
