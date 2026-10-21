// 창 닫기 확인(native.ts 의 `guardClose`): [취소] 하면 닫지 않고 네이티브에 알린다 — 업데이트 뒤 다시 시작이 그것으로 거둬진다
// (src-tauri/src/update.rs).

import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn(async (_command: string) => {});
let onClose: ((event: { preventDefault(): void }) => Promise<void>) | undefined;

vi.mock("@tauri-apps/api/core", () => ({ invoke: (command: string) => invoke(command) }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({
    onCloseRequested: async (handler: typeof onClose) => {
      onClose = handler;
      return () => {};
    },
  }),
}));

const { tauriKeymapNative } = await import("./native");

beforeEach(() => {
  invoke.mockClear();
  onClose = undefined;
});

async function requestClose(shouldClose: boolean) {
  tauriKeymapNative.guardClose!(async () => shouldClose);
  await vi.waitFor(() => expect(onClose).toBeDefined());
  const preventDefault = vi.fn();
  await onClose!({ preventDefault });
  return preventDefault;
}

describe("guardClose", () => {
  it("keeps the window and tells the native side when the close is cancelled", async () => {
    const preventDefault = await requestClose(false);
    expect(preventDefault).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith("window_close_cancelled");
  });

  it("lets the window close without reporting anything", async () => {
    const preventDefault = await requestClose(true);
    expect(preventDefault).not.toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalled();
  });
});
