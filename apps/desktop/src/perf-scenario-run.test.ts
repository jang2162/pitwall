// @vitest-environment happy-dom
import { afterEach, expect, it } from "vitest";

import { pressAndConfirm, watchConsole } from "./perf-scenario-run";

afterEach(() => {
  document.body.replaceChildren();
});

/** 기다림마다 `tick` 을 한 번 부르는 가짜 바깥. */
function fakeEnv(tick: () => void) {
  let t = 0;
  return {
    doc: document,
    now: () => t,
    sleep: async (ms: number) => {
      t += ms;
      tick();
    },
  };
}

it("presses the button and answers the first-run confirmation with its last button", async () => {
  const start = document.createElement("button");
  start.dataset.testid = "run-start";
  document.body.append(start);
  let started = false;
  let asked = false;
  start.addEventListener("click", () => (asked = true));
  const env = fakeEnv(() => {
    if (asked && !document.querySelector("[data-testid=run-confirm-dialog]")) {
      const dialog = document.createElement("div");
      dialog.dataset.testid = "run-confirm-dialog";
      const cancel = document.createElement("button");
      const run = document.createElement("button");
      run.addEventListener("click", () => (started = true));
      dialog.append(cancel, run);
      document.body.append(dialog);
    }
  });
  await pressAndConfirm(env, "run-start", () => started);
  expect(started).toBe(true);
});

it("waits for an enabled button and does not ask when there is no confirmation", async () => {
  const debug = document.createElement("button");
  debug.dataset.testid = "run-debug";
  debug.disabled = true;
  document.body.append(debug);
  let frames = 0;
  let started = false;
  debug.addEventListener("click", () => (started = true));
  const env = fakeEnv(() => {
    if (++frames === 3) debug.disabled = false;
  });
  await pressAndConfirm(env, "run-debug", () => started);
  expect(started).toBe(true);
  expect(frames).toBeGreaterThanOrEqual(3);
});

it("counts rendered output bytes and keeps note lines without their escapes", async () => {
  const pending: (() => void)[] = [];
  const term = {
    write: (_data: string | Uint8Array, cb?: () => void) => void pending.push(() => cb?.()),
  };
  let now = 5;
  const w = watchConsole(term as never, () => now);
  term.write(new Uint8Array(10));
  term.write("\x1b[2m3 MB dropped\x1b[0m\r\n");
  expect(w.counts.rendered).toBe(0);
  for (const f of pending.splice(0)) f();
  expect(w.counts.rendered).toBe(10);
  expect(w.counts.first).toBe(5);
  expect(w.notes).toEqual(["3 MB dropped"]);
  now = 9;
  const flushed = w.flushed();
  for (const f of pending.splice(0)) f();
  await flushed;
  w.restore();
});
