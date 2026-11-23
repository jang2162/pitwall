// 사람 입력 알림(remote-input.ts): 키 · 포인터 입력을 10초에 한 번만 알리고, 해제하면 더 알리지 않는다.

import { expect, it } from "vitest";

import { EVERY_MS, watchInput } from "./remote-input";

it("notes input at most once per interval and stops when disposed", () => {
  const target = new EventTarget() as unknown as Window;
  let clock = 1_000;
  let notes = 0;
  const stop = watchInput(
    target,
    () => notes++,
    () => clock,
  );
  target.dispatchEvent(new Event("keydown"));
  target.dispatchEvent(new Event("pointerdown"));
  expect(notes).toBe(1);
  clock += EVERY_MS - 1;
  target.dispatchEvent(new Event("keydown"));
  expect(notes).toBe(1);
  clock += 1;
  target.dispatchEvent(new Event("pointerdown"));
  expect(notes).toBe(2);
  stop();
  clock += EVERY_MS * 2;
  target.dispatchEvent(new Event("keydown"));
  expect(notes).toBe(2);
});
