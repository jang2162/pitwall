// @vitest-environment happy-dom
// 창을 앞으로 가져오기(./window-focus — 15-m4-plan M4-08 리뷰 M2): 앱이 포커스를 가졌을 때만, 이 창이 이미 포커스를 가졌으면 하지 않는다.

import { afterEach, beforeEach, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({
  focused: [] as boolean[],
  raised: 0,
  label: "project-a",
}));

vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({
    label: tauri.label,
    setFocus: async () => void tauri.raised++,
  }),
  getAllWebviewWindows: async () => tauri.focused.map((f) => ({ isFocused: async () => f })),
}));

const { bringForward, windowLabel } = await import("./window-focus");

let hasFocus: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  tauri.focused = [];
  tauri.raised = 0;
  hasFocus = vi.spyOn(document, "hasFocus").mockReturnValue(false);
});

afterEach(() => {
  vi.restoreAllMocks();
});

it("gives the window label the Host knows the connection by", () => {
  expect(windowLabel()).toBe("project-a");
});

it("brings the window forward when another window of the app has focus", async () => {
  tauri.focused = [false, true];
  await bringForward();
  expect(tauri.raised).toBe(1);
});

it("does not take focus from another app", async () => {
  tauri.focused = [false, false];
  await bringForward();
  expect(tauri.raised).toBe(0);
});

it("does nothing when this window already has focus", async () => {
  hasFocus.mockReturnValue(true);
  tauri.focused = [true];
  await bringForward();
  expect(tauri.raised).toBe(0);
});
