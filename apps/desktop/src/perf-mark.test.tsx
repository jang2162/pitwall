// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { FirstFrameMark, perfMarkRequested } from "./perf-mark";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
});

afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
});

it("only the measurement query asks for the mark", () => {
  expect(perfMarkRequested("?window=project&project=a&perf=1")).toBe(true);
  expect(perfMarkRequested("?window=project&project=a")).toBe(false);
  expect(perfMarkRequested("?window=project&perf=0")).toBe(false);
});

it("marks once, on the second frame after the first commit", () => {
  const frames: (() => void)[] = [];
  const onFrame = vi.fn();
  const frame = (callback: () => void) => void frames.push(callback);
  root = createRoot(document.createElement("div"));
  act(() => root!.render(<FirstFrameMark onFrame={onFrame} frame={frame} />));

  expect(frames).toHaveLength(1);
  frames.shift()!();
  expect(onFrame).not.toHaveBeenCalled();
  expect(frames).toHaveLength(1);
  frames.shift()!();
  expect(onFrame).toHaveBeenCalledTimes(1);
  expect(frames).toHaveLength(0);
});
